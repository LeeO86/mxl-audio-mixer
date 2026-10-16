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
    await page.goto(`${base}/#status`);
    await page.locator('[data-stream="main"] .pill', { hasText: "publishing" }).waitFor({ timeout: 5000 });
    await page.locator(".preview-status .pill", { hasText: /^running, pid/ }).waitFor({ timeout: 5000 });
    await page.screenshot({ path: `${out}/status-preview.png`, fullPage: true });
    await page.goto(`${base}/#control-room`);
    await page.locator(".monitor").getByRole("button", { name: "Listening" }).click();
    await page.locator(".monitor").getByRole("button", { name: "Listen", exact: true }).waitFor({ timeout: 3000 });
    pass("control room: monitor main played in the browser over WHEP (stream publishing), then stopped");
  }

  // Talkback: a short press latches, again releases; a long press talks while held.
  const post = (path, body) => api(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  await post("/api/v1/talkback/mics", { id: "dir", name: "Director", input: 1, channel: 1, priority: 5 });
  await post("/api/v1/talkback/destinations", { id: "aux", name: "Aux talk", bus: "aux", mode: "interrupt", return: { input: 1 } });
  await page.goto(`${base}/#control-room`);
  const talkAux = page.locator(".talkback .tbbuttons").first().getByRole("button", { name: "Aux talk", exact: true });
  const listenAux = page.locator(".talkback .tbbuttons").last().getByRole("button", { name: "Aux talk", exact: true });
  await talkAux.waitFor({ timeout: 5000 });
  const talks = async () => ((await api("/api/v1/talkback")).talking.dir || []).includes("aux");
  await talkAux.click();
  await until("aux latched", talks);
  await page.locator(".monitor .state-tag", { hasText: "talkback" }).waitFor({ timeout: 3000 });
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${out}/talkback.png`, fullPage: true });
  await talkAux.click();
  await until("aux released", async () => !(await talks()));
  // A pointer event on the button itself: a mouse hold misses the button when the runner is busy.
  await talkAux.dispatchEvent("pointerdown", { pointerId: 1, bubbles: true, button: 0, buttons: 1 });
  await until("a held talk button talks", talks, 5000);
  await page.waitForTimeout(500);
  await talkAux.dispatchEvent("pointerup", { pointerId: 1, bubbles: true, button: 0, buttons: 0 });
  await until("held talk released", async () => !(await talks()));
  await listenAux.click();
  await until("listen-back on", async () => (await api("/api/v1/listen-back"))?.input === 1);
  await page.locator(".monitor .state-tag", { hasText: "listen-back" }).waitFor({ timeout: 3000 });
  await listenAux.click();
  await until("listen-back off", async () => (await api("/api/v1/listen-back")) === null);
  pass("talkback: latched by a short press (the monitor's talkback dim shown), released; held talks only while held; listen-back of the return");

  // VCAs and mute groups: made in the groups panel; the VCA fader by keys on both clients; spill; a mute
  // group from the toolbar.
  await page.goto(`${base}/#console`);
  await page.getByRole("button", { name: "VCAs & groups" }).click();
  await page.getByLabel("New VCA id").fill("talent");
  await page.getByLabel("New VCA name").fill("Talent");
  await page.getByRole("button", { name: "+ VCA" }).click();
  await until("VCA talent", async () => (await api("/api/v1/vcas")).some((v) => v.id === "talent"));
  await page.getByLabel("Vox in Talent").check();
  await until("vox in talent", async () => (await api("/api/v1/vcas/talent")).members.includes("vox"));
  await page.locator('[data-vca="talent"] .vtrack').focus();
  for (let i = 0; i < 4; i++) await page.keyboard.press("ArrowDown");
  await until("VCA at −2 dB", async () => (await api("/api/v1/vcas/talent")).fader_db === -2);
  await other.locator('[data-vca="talent"] .vvalue', { hasText: "−2.0" }).waitFor({ timeout: 3000 });
  await page.getByRole("button", { name: "Spill Talent" }).click();
  await page.locator(".spillbanner").waitFor({ timeout: 3000 });
  const spilledStrips = await page.locator(".strips .strip").count();
  if (spilledStrips !== 1 || !(await page.locator('.strips [data-signal="vox"]').count())) throw new Error(`spill shows ${spilledStrips} strips`);
  await page.getByLabel("New mute group id").fill("band");
  await page.getByLabel("New mute group name").fill("Band");
  await page.getByRole("button", { name: "+ Mute group" }).click();
  await until("mute group band", async () => (await api("/api/v1/mute-groups")).some((g) => g.id === "band"));
  await page.getByLabel("Guitar in Band").check();
  await until("gtr in band", async () => (await api("/api/v1/mute-groups/band")).members.includes("gtr"));
  await page.screenshot({ path: `${out}/vca-groups.png`, fullPage: true });
  await page.locator(".spillbanner").getByRole("button", { name: "Show all" }).click();
  await page.getByRole("button", { name: "Close" }).click();
  await page.locator(".mutegroups").getByRole("button", { name: "Band" }).click();
  await until("band active", async () => (await api("/api/v1/mute-groups/band")).active === true);
  await page.locator('[data-signal="gtr"] .btn.mute.othermute').waitFor({ timeout: 3000 });
  if ((await api("/api/v1/strips/gtr")).mute !== false) throw new Error("the mute group changed gtr's own mute");
  await page.screenshot({ path: `${out}/console-vca.png`, fullPage: true });
  await page.locator(".mutegroups").getByRole("button", { name: "Band" }).click();
  await until("band inactive", async () => (await api("/api/v1/mute-groups/band")).active === false);
  pass("VCAs and mute groups: VCA made and assigned in the panel, its fader by keys on both clients, spill, a mute group from the toolbar (gtr's own mute unchanged)");

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

  // Graph: drag from gtr to aux to send, click the send to edit it, Delete removes it.
  await page.goto(`${base}/#graph`);
  await page.locator('.vue-flow__node[data-id="gtr"]').waitFor({ timeout: 5000 });
  await page.waitForTimeout(800);
  const center = async (sel) => {
    const b = await page.locator(sel).boundingBox();
    return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
  };
  const from = await center('.vue-flow__node[data-id="gtr"] .vue-flow__handle.source');
  const to = await center('.vue-flow__node[data-id="aux"] .vue-flow__handle.target');
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(to.x, to.y, { steps: 12 });
  await page.mouse.up();
  await until("send gtr → aux from the graph", async () => (await api("/api/v1/sends")).some((x) => x.from === "gtr" && x.to === "aux"));
  const edge = page.locator('.vue-flow__edge[data-id="send:gtr:aux"]');
  await edge.waitFor({ timeout: 3000 });
  // A straight edge's path has no height: click its middle.
  const box = await edge.locator(".vue-flow__edge-interaction").boundingBox();
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  await page.locator("[data-send-edit]").waitFor({ timeout: 3000 });
  await page.screenshot({ path: `${out}/graph.png`, fullPage: true });
  await page.keyboard.press("Delete");
  await until("send gtr → aux removed by Delete", async () => !(await api("/api/v1/sends")).some((x) => x.from === "gtr" && x.to === "aux"));
  pass("graph: a send drawn from Guitar to Aux, edited from its edge, removed with Delete");

  // Operations: N-1s of Main for the selection, made in the operations panel.
  await page.goto(`${base}/#console`);
  await page.getByRole("button", { name: "Operations" }).click();
  await page.getByLabel("Select Vox").check();
  await page.getByLabel("Select Guitar").check();
  await page.getByLabel("N-1 of").selectOption("main");
  await page.getByRole("button", { name: "Create N-1" }).click();
  await page.locator("[data-op-result]", { hasText: "n1-vox" }).waitFor({ timeout: 5000 });
  const n1 = (await api("/api/v1/buses")).find((b) => b.id === "n1-vox");
  if (n1?.minus?.of !== "main" || n1.minus.without.join() !== "vox") throw new Error(`n1-vox: ${JSON.stringify(n1)}`);
  await page.locator('[data-signal="n1-vox"] .minus').waitFor({ timeout: 3000 });
  await page.screenshot({ path: `${out}/operations.png`, fullPage: true });
  for (const b of ["n1-vox", "n1-gtr"]) await api(`/api/v1/buses/${b}`, { method: "DELETE" });
  pass("operations: N-1s of Main for Vox and Guitar from the panel (mix-minus buses, shown on the console)");

  // Auto: an auto-mix of Vox and Guitar, a weight, a duck of Main under Vox. (Vox has an EQ boost near
  // its 1 kHz tone from the processing steps: it gets the larger share.)
  await page.getByRole("button", { name: "Operations" }).click();
  await page.getByRole("button", { name: "Auto", exact: true }).click();
  await page.getByLabel("New auto-mix name").fill("Panel");
  await page.getByRole("button", { name: "+ Auto-mix" }).click();
  await page.getByLabel("Vox in Panel").check();
  await until("Vox in the auto-mix", async () => (await api("/api/v1/automixes"))[0]?.members.length === 1);
  await page.getByLabel("Guitar in Panel").check();
  await until("Guitar in the auto-mix", async () => (await api("/api/v1/automixes"))[0]?.members.length === 2);
  const shares = async () => {
    const g = (await api("/api/v1/automixes"))[0].gains_db;
    return g.vox !== undefined && g.gtr !== undefined ? g : null;
  };
  const shown = (id) => page.locator(`[data-signal="${id}"] .autogain`).getAttribute("data-auto");
  const before = await until("both members share", async () => {
    const g = await shares();
    return g && (await shown("vox")) === g.vox.toFixed(1) && (await shown("gtr")) === g.gtr.toFixed(1) ? g : null;
  });
  const power = 10 ** (before.vox / 10) + 10 ** (before.gtr / 10);
  if (Math.abs(power - 1) > 0.01) throw new Error(`the shares' powers add up to ${power}`);
  await page.locator('[data-signal="vox"] .vad[data-vad="quiet"]').waitFor({ timeout: 3000 });
  await other.locator('[data-signal="gtr"] .autogain').waitFor({ timeout: 3000 });
  // Tones are not speech: with last-mic hold the gains would stay where they are.
  await page.locator("[data-automix-edit]").getByLabel("last-mic hold").uncheck();
  await until("last-mic hold off", async () => (await api("/api/v1/automixes"))[0].last_mic_hold === false);
  // Typed at a person's pace: the meters (25 Hz) must not reset the field meanwhile.
  await page.getByLabel("Guitar weight").selectText();
  await page.getByLabel("Guitar weight").pressSequentially("-6", { delay: 150 });
  await page.waitForTimeout(400);
  await page.getByLabel("Guitar weight").press("Enter");
  await until("Guitar weight −6", async () => (await api("/api/v1/automixes"))[0].members.find((m) => m.signal === "gtr")?.weight_db === -6);
  const after = await until("Guitar's share down", async () => {
    const g = await shares();
    return g && g.gtr < before.gtr - 3 && (await shown("gtr")) === g.gtr.toFixed(1) ? g : null;
  });
  pass(`auto-mix of Vox and Guitar from the panel: ${before.vox} and ${before.gtr} dB on both clients (powers add up to 1); Guitar's weight −6 dB: ${after.vox} and ${after.gtr} dB; the VAD dot quiet on a tone`);
  await page.getByLabel("New duck name").fill("Bed");
  await page.getByRole("button", { name: "+ Duck" }).click();
  await page.getByLabel("Vox keys Bed").check();
  await page.getByLabel("Main ducked by Bed").check();
  await until("duck Bed: Vox keys Main", async () => {
    const d = (await api("/api/v1/ducks"))[0];
    return d?.keys.join() === "vox" && d.targets.join() === "main";
  });
  await page.locator('[data-signal="main"] .autogain[data-auto="0.0"]').waitFor({ timeout: 5000 });
  await page.locator('[data-duck-edit] [data-duck-active="false"]').waitFor({ timeout: 3000 });
  pass("duck from the panel: Main under Vox's speech (none on a tone: Main at 0 dB)");
  // The duck as a ratio duck (Com-IS-Assist's control), a ratio of 8 LU, the LFE's share 0.
  await page.getByLabel("Bed mode").selectOption("ratio");
  await page.getByLabel("Bed ratio", { exact: true }).fill("8");
  await page.getByLabel("Bed ratio", { exact: true }).press("Enter");
  await page.getByLabel("Bed LFE share").fill("0");
  await page.getByLabel("Bed LFE share").press("Enter");
  const ratio = await until("Bed: ratio 8 LU, LFE 0", async () => {
    const d = (await api("/api/v1/ducks"))[0];
    return d.mode === "ratio" && d.ratio.target_lu === 8 && d.ratio.overvoice_lu === 13 && d.channel_depth.lfe === 0 && d.channel_depth.front === 1 ? d : null;
  });
  await page.locator('[data-duck-edit] [data-duck-state="release"]').waitFor({ timeout: 3000 });
  await page.screenshot({ path: `${out}/automation.png`, fullPage: true });
  const am = (await api("/api/v1/automixes"))[0];
  await api(`/api/v1/automixes/${am.id}`, { method: "DELETE" });
  await api(`/api/v1/ducks/${ratio.id}`, { method: "DELETE" });
  pass("the duck as a ratio duck from the panel: ratio 8 LU (over-voice kept at 13), the LFE's share 0; released on a tone");
  // Auto-level Guitar to −30 LUFS; the strip shows the gain the controller reports.
  await page.getByLabel("Signal to auto-level").selectOption("gtr");
  await page.getByRole("button", { name: "+ Auto-level" }).click();
  await page.getByLabel("Guitar target").selectText();
  await page.getByLabel("Guitar target").pressSequentially("-30", { delay: 150 });
  await page.getByLabel("Guitar target").press("Enter");
  const level = await until("Guitar auto-levelled to −30", async () => {
    const l = await api("/api/v1/autolevels/gtr").catch(() => null);
    if (l?.target_lufs !== -30 || l.state?.level_lufs === null || l.state?.level_lufs === undefined) return null;
    const want = Math.max(l.min_gain_db, Math.min(l.max_gain_db, -30 - l.state.level_lufs));
    return Math.abs(l.state.gain_db - want) < 0.3 && (await shown("gtr")) === l.state.gain_db.toFixed(1) ? l.state : null;
  }, 20000);
  await page.locator('[data-level="gtr"]').waitFor({ timeout: 3000 });
  await page.screenshot({ path: `${out}/autolevel.png`, fullPage: true });
  await api("/api/v1/autolevels/gtr", { method: "DELETE" });
  pass(`auto-level from the panel: Guitar at ${level.level_lufs} LUFS to −30, gain ${level.gain_db} dB, shown on its strip`);

  // Logic: a constant 1 wired to "set mute" mutes Guitar.
  await page.goto(`${base}/#logic`);
  await page.getByLabel("New logic graph name").fill("Keys");
  await page.getByRole("button", { name: "+ Graph" }).click();
  await page.getByLabel("Block type").waitFor({ timeout: 5000 });
  const graph = await until("graph Keys", async () => (await api("/api/v1/logic")).find((g) => g.name === "Keys"));
  // A new graph starts in dry run; this one goes live.
  await page.getByLabel("dry run").uncheck();
  await until("Keys live", async () => (await api(`/api/v1/logic/${graph.id}`)).dry_run === false);
  await page.getByLabel("Block type").selectOption("const");
  await page.getByRole("button", { name: "+ Block" }).click();
  await page.getByLabel("Block type").selectOption("set-mute");
  await page.getByLabel("Block signal").selectOption("gtr");
  await page.getByRole("button", { name: "+ Block" }).click();
  await until("two blocks", async () => (await api(`/api/v1/logic/${graph.id}`)).blocks.length === 2);
  await page.getByLabel("const-1 value").fill("1");
  await page.getByLabel("const-1 value").press("Enter");
  await until("const is 1", async () => (await api(`/api/v1/logic/${graph.id}`)).blocks.find((b) => b.id === "const-1").value === 1);
  await page.locator('.vue-flow__node[data-id="set-mute-1"]').waitFor({ timeout: 5000 });
  await page.getByRole("button", { name: "Fit" }).click();
  await page.waitForTimeout(400);
  const cableFrom = await center('.vue-flow__node[data-id="const-1"] .vue-flow__handle.source');
  const cableTo = await center('.vue-flow__node[data-id="set-mute-1"] .vue-flow__handle.target');
  await page.mouse.move(cableFrom.x, cableFrom.y);
  await page.mouse.down();
  await page.mouse.move(cableTo.x, cableTo.y, { steps: 12 });
  await page.mouse.up();
  await until("Guitar muted by the cable", async () => (await api("/api/v1/strips/gtr")).mute === true);
  await page.screenshot({ path: `${out}/logic.png`, fullPage: true });
  const cable = page.locator(".vue-flow__edge").first();
  const cableBox = await cable.locator(".vue-flow__edge-interaction").boundingBox();
  await page.mouse.click(cableBox.x + cableBox.width / 2, cableBox.y + cableBox.height / 2);
  await page.keyboard.press("Delete");
  await until("the cable removed", async () => (await api(`/api/v1/logic/${graph.id}`)).wires.length === 0);
  // Parameters are edited in the list: on air index, timer, meter threshold, snapshot recall.
  const blockOf = async (type) => (await api(`/api/v1/logic/${graph.id}`)).blocks.find((x) => x.type === type);
  for (const type of ["tally", "timer", "above", "recall"]) {
    if (type === "recall") await api("/api/v1/snapshots", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id: "logic-show", name: "Logic" }) });
    await page.getByLabel("Block type").selectOption(type);
    await page.getByRole("button", { name: "+ Block" }).click();
    await until(`a ${type} block`, () => blockOf(type));
  }
  const setField = async (label, value) => {
    await page.getByLabel(label, { exact: true }).fill(value);
    await page.getByLabel(label, { exact: true }).press("Enter");
  };
  await setField("tally-1 index", "5");
  await until("tally index 5", async () => (await blockOf("tally"))?.index === 5);
  await page.locator('.vue-flow__node[data-id="tally-1"]').getByText(": 5").waitFor({ timeout: 5000 });
  await setField("timer-1 ms", "500");
  await until("timer 500 ms", async () => (await blockOf("timer"))?.ms === 500);
  await page.locator('.vue-flow__node[data-id="timer-1"]').getByText("500 ms").waitFor({ timeout: 5000 });
  await setField("above-1 threshold_db", "-40");
  await setField("above-1 hysteresis_db", "6");
  await until("above -40/6", async () => {
    const b = await blockOf("above");
    return b?.threshold_db === -40 && b?.hysteresis_db === 6;
  });
  // A new recall has no snapshot until one is chosen.
  if ((await blockOf("recall"))?.snapshot !== "") throw new Error("a recall got a snapshot by itself");
  await until("snapshot in the list", async () => (await page.getByLabel("recall-1 snapshot", { exact: true }).innerHTML()).includes("logic-show"));
  await page.getByLabel("recall-1 snapshot", { exact: true }).selectOption("logic-show");
  await until("recall logic-show", async () => (await blockOf("recall"))?.snapshot === "logic-show");
  await api(`/api/v1/logic/${graph.id}`, { method: "DELETE" });
  await api("/api/v1/snapshots/logic-show", { method: "DELETE" });
  await api("/api/v1/strips/gtr", { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ mute: false }) });
  pass("logic: a cable drawn from a constant to set-mute mutes Guitar, Delete removes it, and on-air and timer blocks take an index and a duration");

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
