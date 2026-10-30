// Browser check of the console's 1.5.0 items (A1-A6, A8-A10) against a running mixer
// (tests/integration/ui.sh): the tab title, the strip head, typing a fader level, the sends as values, the
// muted meter, wide strips and meters, and the surround panner in the strip (front/rear, height). Every
// action through the UI, checked on the API. Screenshots go to <out dir>.
//
// node web/tests/console15.mjs <http base> <out dir>
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
const send = (method, path, body) => api(path, { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body ?? {}) });
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
const pass = (what) => console.log(`[console15.mjs] PASS: ${what}`);

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
const made = { strips: [], buses: [] };
try {
  await send("POST", "/api/v1/buses", { id: "it-imm", name: "Immersive", format: "7.1.4" });
  made.buses.push("it-imm");
  await send("POST", "/api/v1/strips", { id: "it-bed", name: "Bed 7.1.4", format: "7.1.4" });
  made.strips.push("it-bed");
  await send("POST", "/api/v1/strips", { id: "it-fx", name: "FX", format: "stereo", patch: [{ input: 1, channel: 1 }, { input: 1, channel: 2 }] });
  made.strips.push("it-fx");
  await send("PUT", "/api/v1/sends/it-fx/it-imm");

  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#console`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  const strip = (id) => page.locator(`[data-signal="${id}"]`);
  await strip("it-fx").waitFor();

  // A1: the tab title names the tab and the node.
  const label = (await api("/api/v1/status")).label;
  await until("console title", async () => (await page.title()) === `Console – ${label} – MXL Audio Mixer`);
  await page.locator("nav button", { hasText: "Processing" }).click();
  await until("processing title", async () => (await page.title()).startsWith("Processing – "));
  await page.locator("nav button", { hasText: "Console" }).click();
  pass(`A1: tab title "${await page.title()}", follows the tab`);

  // A4: the name has the whole width; gear and menu sit in the format row.
  await strip("vox").locator(".sfmt .sicon.gear").waitFor();
  await strip("vox").locator(".sfmt .fmt", { hasText: "mono" }).waitFor();
  const head = await strip("vox").locator(".shead .sname").boundingBox();
  const whole = await strip("vox").locator(".shead").boundingBox();
  if (head.width < whole.width - 2) throw new Error(`name ${head.width} px of ${whole.width} px`);
  pass("A4: the name has the strip's width, gear and ⋮ in the format row");

  // A5: type a fader level.
  const typeLevel = async (id, text) => {
    await strip(id).locator("button.vvalue").click();
    const field = strip(id).locator("input.vtype");
    await field.fill(text);
    await field.press("Enter");
  };
  await typeLevel("vox", "-20");
  await until("vox at −20 dB", async () => (await api("/api/v1/strips/vox")).fader_db === -20);
  await typeLevel("vox", "−3,5");
  await until("vox at −3.5 dB", async () => (await api("/api/v1/strips/vox")).fader_db === -3.5);
  await typeLevel("vox", "-inf");
  await until("vox at −∞", async () => (await api("/api/v1/strips/vox")).fader_db === "-inf");
  await typeLevel("vox", "0");
  await until("vox at 0 dB", async () => (await api("/api/v1/strips/vox")).fader_db === 0);
  await typeLevel("vox", "loud");
  await page.waitForTimeout(300);
  if ((await api("/api/v1/strips/vox")).fader_db !== 0) throw new Error("text that is no level changed the fader");
  await strip("vox").locator("button.vvalue").click();
  await strip("vox").locator("input.vtype").fill("-12");
  await strip("vox").locator("input.vtype").press("Escape");
  await page.waitForTimeout(300);
  if ((await api("/api/v1/strips/vox")).fader_db !== 0) throw new Error("Escape changed the fader");
  pass("A5: typed levels -20, −3,5, -inf, 0; text that is no level and Escape change nothing");

  // A6: the sends show values only; a click opens Routing.
  await send("PUT", "/api/v1/sends/vox/main", { gain_db: -6 });
  await strip("vox").locator('.sendval[data-send="main"] .num', { hasText: "−6.0" }).waitFor();
  if (await strip("vox").locator('.sends input[type="range"]').count()) throw new Error("a send fader on the strip");
  await strip("vox").locator('.sendval[data-send="main"]').click();
  await until("routing tab", async () => (await page.evaluate(() => location.hash)) === "#routing");
  await page.locator("nav button", { hasText: "Console" }).click();
  await send("PUT", "/api/v1/sends/vox/main", { gain_db: 0 });
  pass("A6: the strip shows the send level (−6.0) without a fader; a click opens Routing");

  // A3: a muted strip's meter still moves, grey.
  await send("PATCH", "/api/v1/strips/vox", { mute: true });
  await strip("vox").locator(".meter.muted").waitFor();
  await until("the muted meter shows a level", async () => {
    const h = await strip("vox").locator(".meter .lvl").first().evaluate((e) => e.getBoundingClientRect().height);
    return h > 10 ? h : null;
  });
  const grey = await strip("vox").locator(".meter .lvl").first().evaluate((e) => getComputedStyle(e).backgroundColor);
  await strip("vox").screenshot({ path: `${out}/console15-muted.png` });
  await send("PATCH", "/api/v1/strips/vox", { mute: false });
  await until("unmuted meter", async () => !(await strip("vox").locator(".meter.muted").count()));
  pass(`A3: muted vox still meters its level, in ${grey}`);

  // A2, A9: a 7.1.4 strip is wider and keeps a usable fader; the meter bridge has narrow bars with labels.
  await strip("it-bed").locator(".meter.wide").waitFor();
  const bed = await strip("it-bed").boundingBox();
  const track = await strip("it-bed").locator(".vtrack").boundingBox();
  const bars = await strip("it-bed").locator(".meter .bar").count();
  if (bed.width < 120 || track.width < 40 || bars !== 12) throw new Error(`7.1.4 strip ${bed.width} px, fader ${track.width} px, ${bars} bars`);
  await strip("it-bed").screenshot({ path: `${out}/console15-714-strip.png` });
  await page.locator("nav button", { hasText: "Meters" }).click();
  const col = page.locator(".bridge .mcol", { hasText: "Bed 7.1.4" });
  await col.locator(".chlabels span").first().waitFor();
  const labels = await col.locator(".chlabels span").allTextContents();
  if (labels.join(" ") !== "L R C LFE Lss Rss Lrs Rrs Ltf Rtf Ltr Rtr") throw new Error(`labels ${labels.join(" ")}`);
  const barW = await col.locator(".meter .bar").first().evaluate((e) => e.getBoundingClientRect().width);
  if (barW > 7.5) throw new Error(`bridge bar ${barW} px`);
  await page.locator(".bridge").screenshot({ path: `${out}/console15-bridge.png` });
  await page.locator("nav button", { hasText: "Console" }).click();
  pass(`A2, A9: 7.1.4 strip ${Math.round(bed.width)} px with a ${Math.round(track.width)} px fader; bridge bars ${barW} px, labels below`);

  // A10, A8: FX sends to a 7.1.4 bus: a surround panner from the strip, moved on its picture, lifted.
  await strip("it-fx").getByRole("button", { name: "+ Surround pan" }).click();
  const node = await until("a 7.1.4 surround panner on FX", async () => (await api("/api/v1/strips/it-fx/chain")).find((n) => n.type === "surround-pan" && n.layout === "7.1.4"));
  const pad = strip("it-fx").locator(".spanmini svg");
  await pad.waitFor();
  const box = await pad.boundingBox();
  await pad.click({ position: { x: 4, y: box.height / 2 } });
  await until("FX panned left", async () => {
    const n = (await api("/api/v1/strips/it-fx/chain")).find((x) => x.id === node.id);
    return n.azimuth_deg < -80 && n.azimuth_deg > -100 ? n : null;
  });
  await pad.click({ position: { x: box.width / 2, y: box.height - 3 } });
  await until("FX panned to the rear", async () => Math.abs((await api("/api/v1/strips/it-fx/chain")).find((x) => x.id === node.id).azimuth_deg) > 170);
  await strip("it-fx").locator(".spanmini input[type=range]").fill("45");
  await until("FX 45° up", async () => (await api("/api/v1/strips/it-fx/chain")).find((x) => x.id === node.id).elevation_deg === 45);
  await page.waitForTimeout(500);
  const ready = (await api("/api/v1/status")).readiness;
  if (!ready.engine_running) throw new Error("the DSP stopped with a stereo 7.1.4 panner");
  await strip("it-fx").screenshot({ path: `${out}/console15-surround.png` });
  pass("A8, A10: + Surround pan into 7.1.4 from the strip, left and rear on its picture, height 45°; the DSP runs");

  await page.screenshot({ path: `${out}/console15.png` });
  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
  pass("no page errors");
} finally {
  for (const id of made.strips) await fetch(`${base}/api/v1/strips/${id}`, { method: "DELETE" }).catch(() => {});
  for (const id of made.buses) await fetch(`${base}/api/v1/buses/${id}`, { method: "DELETE" }).catch(() => {});
  await browser.close();
}
