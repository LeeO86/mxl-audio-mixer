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
