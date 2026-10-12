// Browser check of the web UI against a running mixer (tests/integration/ui.sh sets up the show): two
// clients in headless Chrome (playwright-core, the system's Chrome), every action through the UI and
// checked on the API, the other client following. Screenshots of every tab go to <out dir>.
//
// node web/tests/ui.mjs <http base> <out dir>
import { mkdirSync } from "node:fs";
import { chromium } from "playwright-core";

const [base, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const executablePath = process.env.CHROME || "/usr/bin/google-chrome";
const api = async (path, init) => {
  const r = await fetch(base + path, init);
  if (!r.ok) throw new Error(`${path}: HTTP ${r.status}`);
  return r.status === 204 ? null : r.json();
};
async function until(what, fn, ms = 5000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try {
      last = await fn();
      if (last) return last;
    } catch (e) {
      last = e.message;
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`timed out: ${what} (last: ${JSON.stringify(last)})`);
}
const pass = (what) => console.log(`[ui.mjs] PASS: ${what}`);

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
try {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#console`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  await page.locator('[data-signal="vox"]').waitFor();
  const strips = await page.locator(".strips .strip").count();
  if (strips !== 2) throw new Error(`${strips} strips on the console`);
  pass("console: live, 2 strips and the buses");

  // A second client follows every change.
  const other = await (await browser.newContext({ viewport: { width: 1280, height: 900 } })).newPage();
  await other.goto(`${base}/#console`);
  await other.locator('[data-signal="vox"]').waitFor();

  await page.locator('[data-signal="vox"] .btn.mute').click();
  await until("vox muted on the API", async () => (await api("/api/v1/strips/vox")).mute === true);
  await other.locator('[data-signal="vox"] .btn.mute.danger').waitFor({ timeout: 3000 });
  await page.locator('[data-signal="vox"] .btn.mute').click();
  await until("vox unmuted", async () => (await api("/api/v1/strips/vox")).mute === false);
  pass("mute through the UI; the second client shows it");
  // Two quick presses toggle twice, whatever the server has answered meanwhile.
  await page.locator('[data-signal="gtr"] .btn.mute').click();
  await page.locator('[data-signal="gtr"] .btn.mute').click();
  await page.waitForTimeout(600);
  await until("two quick mute presses to leave gtr unmuted", async () => (await api("/api/v1/strips/gtr")).mute === false, 3000);
  pass("two quick mute presses: unmuted again");
  // The same with the mixes pushed to this client 1 s late: the mix after another change (vox) arrives
  // between the two presses, before the server has the first one, and must not undo it.
  const slow = await (await browser.newContext({ viewport: { width: 1280, height: 900 } })).newPage();
  await slow.routeWebSocket(/\/api\/v1\/events/, (ws) => {
    const server = ws.connectToServer();
    server.onMessage((m) => (JSON.parse(String(m)).type === "mix" ? setTimeout(() => ws.send(m), 1000) : ws.send(m)));
    ws.onMessage((m) => server.send(m));
  });
  await slow.goto(`${base}/#console`);
  await slow.locator('[data-signal="gtr"]').waitFor();
  await slow.waitForTimeout(1500);
  await api("/api/v1/strips/vox", { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ mute: true }) });
  await slow.waitForTimeout(300);
  await slow.locator('[data-signal="gtr"] .btn.mute').click();
  await slow.waitForTimeout(850);
  await slow.locator('[data-signal="gtr"] .btn.mute').click();
  await slow.waitForTimeout(1500);
  await until("gtr unmuted after two presses around a late mix", async () => (await api("/api/v1/strips/gtr")).mute === false, 2000);
  await api("/api/v1/strips/vox", { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ mute: false }) });
  await slow.context().close();
  pass("two mute presses with a late mix in between: unmuted again");

  await page.locator('[data-signal="vox"] .vtrack').focus();
  for (let i = 0; i < 4; i++) await page.keyboard.press("ArrowDown");
  await until("vox fader at −2 dB", async () => (await api("/api/v1/strips/vox")).fader_db === -2);
  await other.locator('[data-signal="vox"] .vvalue', { hasText: "−2.0" }).waitFor({ timeout: 3000 });
  const cap = await page.locator('[data-signal="vox"] .vtrack .cap').boundingBox();
  await page.mouse.move(cap.x + cap.width / 2, cap.y + cap.height / 2);
  await page.mouse.down();
  await page.mouse.move(cap.x + cap.width / 2, cap.y + cap.height / 2 + 40, { steps: 5 });
  await page.mouse.up();
  const dragged = await until("vox fader dragged down", async () => {
    const f = (await api("/api/v1/strips/vox")).fader_db;
    return f !== -2 && (f === "-inf" || f < -2) ? f : null;
  });
  pass(`fader: arrow keys to −2 dB, then dragged to ${dragged} dB; the second client follows`);
  await page.locator('[data-signal="vox"] .sends input[type="range"]').first().fill("-6");
  await until("send vox → main at −6 dB", async () => (await api("/api/v1/sends/vox/main")).gain_db === -6);
  pass("send level from the strip");
  await page.screenshot({ path: `${out}/console.png` });

  // Processing: open vox, add an EQ from the palette, drag its band handle up.
  await page.locator('[data-signal="vox"] .sname').click();
  await page.locator(".palette .pchip", { hasText: /^EQ/ }).click();
  await until("an EQ on vox", async () => (await api("/api/v1/strips/vox/chain")).length === 1);
  const handle = page.locator(".nodecard .curve .handle circle").first();
  await handle.waitFor();
  const h = await handle.boundingBox();
  await page.mouse.move(h.x + h.width / 2, h.y + h.height / 2);
  await page.mouse.down();
  await page.mouse.move(h.x + h.width / 2 + 30, h.y + h.height / 2 - 40, { steps: 6 });
  await page.mouse.up();
  const band = await until("EQ band moved", async () => {
    const b = (await api("/api/v1/strips/vox/chain"))[0].bands[0];
    return b.gain_db > 3 && b.freq_hz > 1000 ? b : null;
  });
  pass(`processing: EQ added from the palette, band dragged to ${band.freq_hz} Hz ${band.gain_db} dB`);
  await page.locator(".palette .pchip", { hasText: /^Compressor/ }).click();
  await until("a compressor", async () => (await api("/api/v1/strips/vox/chain")).length === 2);
  await page.locator(".nodecard").nth(1).locator(".btn.bypass").click();
  await until("compressor bypassed", async () => (await api("/api/v1/strips/vox/chain"))[1].bypass === true);
  await page.locator(".nodecard").nth(1).locator("button", { hasText: "↑" }).click();
  await until("compressor first", async () => (await api("/api/v1/strips/vox/chain"))[0].type === "compressor");
  await page.screenshot({ path: `${out}/processing.png`, fullPage: true });
  pass("processing: compressor added, bypassed and moved up");

  // Tally and AFV: index 5 in the strip settings, AFV on, then index 5 on air over TSL (UDP).
  await page.getByLabel("Tally index").fill("5");
  await page.getByLabel("Tally index").press("Tab");
  await until("vox tally index 5", async () => (await api("/api/v1/strips/vox")).tally_index === 5);
  await page.getByLabel("Audio follows video").check();
  await until("vox AFV on", async () => (await api("/api/v1/strips/vox")).afv?.enabled === true);
  const { createSocket } = await import("node:dgram");
  // TSL 5.0: PBC, version, flags (unicode), screen; then index, control (LH red, brightness 3), label.
  const label = Buffer.from("Cam 5", "utf16le");
  const packet = Buffer.alloc(12 + label.length);
  packet.writeUInt16LE(packet.length - 2, 0);
  packet.writeUInt8(0, 2);
  packet.writeUInt8(1, 3);
  packet.writeUInt16LE(0, 4);
  packet.writeUInt16LE(5, 6);
  packet.writeUInt16LE((1 << 4) | (3 << 6), 8);
  packet.writeUInt16LE(label.length, 10);
  label.copy(packet, 12);
  const udp = createSocket("udp4");
  await new Promise((r) => udp.send(packet, Number(process.env.TSL_UDP), "127.0.0.1", r));
  udp.close();
  await until("vox AFV open", async () => (await api("/api/v1/tally")).strips?.vox?.afv_open === true);
  await page.screenshot({ path: `${out}/processing-tally.png` });
  await page.goto(`${base}/#console`);
  await page.locator('[data-signal="vox"].onair').waitFor({ timeout: 3000 });
  await other.locator('[data-signal="vox"].onair').waitFor({ timeout: 3000 });
  await page.goto(`${base}/#nmos`);
  await page.locator("td", { hasText: "Cam 5" }).waitFor({ timeout: 3000 });
  await page.screenshot({ path: `${out}/nmos-tally.png`, fullPage: true });
  pass("tally: index and AFV set in the strip settings; on air over TSL: AFV open, the strip lit on both clients, the lamp listed");

  // Control room: a monitor on main, PFL of vox from its strip, dim from the monitor card.
  await api("/api/v1/monitors", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id: "main", name: "Main", format: "stereo", source: { signal: "main" } }) });
  await page.goto(`${base}/#console`);
  await page.getByRole("button", { name: "PFL Vox" }).click();
  await until("vox on PFL", async () => (await api("/api/v1/solo")).vox === "pfl");
  await page.goto(`${base}/#control-room`);
  await page.locator(".soloed .chip", { hasText: "PFL Vox" }).waitFor({ timeout: 3000 });
  await page.locator(".monitor").getByRole("button", { name: "Dim" }).click();
  await until("main dimmed", async () => (await api("/api/v1/monitors/main")).dim === true);
  await page.waitForTimeout(500);
  await page.screenshot({ path: `${out}/control-room.png`, fullPage: true });
  await page.getByRole("button", { name: "Clear solos" }).click();
  await until("no solo", async () => Object.keys(await api("/api/v1/solo")).length === 0);
  pass("control room: PFL from the strip shown as soloed, monitor dim from its card, solos cleared");

  // Listening in the browser (WHEP from the built-in MediaMTX), only with MediaMTX.
  if (process.env.MEDIAMTX_BIN) {
    const listen = page.locator(".monitor").getByRole("button", { name: "Listen", exact: true });
    await listen.click();
    await page.locator(".monitor").getByRole("button", { name: "Listening" }).waitFor({ timeout: 15000 });
    const stream = async () => (await api("/api/v1/preview")).streams.find((s) => s.id === "main");
    await until("main publishing", async () => (await stream()).state === "publishing" && (await stream()).frames > 0);
    await page.screenshot({ path: `${out}/control-room-listening.png`, fullPage: true });
    await page.locator(".monitor").getByRole("button", { name: "Listening" }).click();
    await page.locator(".monitor").getByRole("button", { name: "Listen", exact: true }).waitFor({ timeout: 3000 });
    pass("control room: monitor main played in the browser over WHEP (stream publishing), then stopped");
  }

  // Routing: a send from vox to aux through the matrix.
  await page.goto(`${base}/#routing`);
  await page.getByRole("button", { name: "Vox to Aux" }).click();
  await until("send vox → aux", async () => (await api("/api/v1/sends")).some((s) => s.from === "vox" && s.to === "aux"));
  await page.screenshot({ path: `${out}/routing.png`, fullPage: true });
  pass("routing: send created in the matrix");

  // Snapshots.
  await page.goto(`${base}/#snapshots`);
  await page.getByLabel("Snapshot name").fill("UI test");
  await page.getByRole("button", { name: "Take snapshot" }).click();
  await until("snapshot taken", async () => (await api("/api/v1/snapshots")).some((s) => s.name === "UI test"));
  await page.screenshot({ path: `${out}/snapshots.png`, fullPage: true });
  pass("snapshots: taken through the UI");

  // Meters: analyser on vox.
  await page.goto(`${base}/#meters`);
  await page.getByLabel("Signal to analyse").selectOption("vox");
  await page.getByRole("button", { name: "Analyse" }).click();
  await until("spectrum bars", async () => (await page.locator(".spectrum rect").count()) > 50);
  await page.waitForTimeout(500);
  await page.screenshot({ path: `${out}/meters.png`, fullPage: true });
  pass("meters: analyser spectrum and goniometer");

  for (const tab of ["nmos", "status", "settings"]) {
    await page.goto(`${base}/#${tab}`);
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${out}/${tab}.png`, fullPage: true });
  }

  await page.locator("header button[aria-label^='Theme']").click();
  const theme = await page.evaluate(() => document.documentElement.dataset.theme);
  if (theme !== "light") throw new Error(`theme ${theme}`);
  await page.goto(`${base}/#console`);
  await page.locator('[data-signal="vox"]').waitFor();
  await page.screenshot({ path: `${out}/console-light.png` });
  pass("theme: switched to light and remembered");

  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
  pass("ui");
} finally {
  await browser.close();
}
