// Browser check of the 1.4.0 DSP features (D1–D5, C10/P3) against a running mixer (tests/integration/ui.sh
// sets up the show; this adds a stereo strip "Music" and a 5.1 bus "Surround"): the format selector of
// the Processing view, the M/S / width node and its flag on a format it cannot run on, the EQ's high-
// and low-pass bands, the surround panner's pad, and the down-mix in Settings and on a bus. Every
// action goes through the UI and is checked on the API. Screenshots go to <out dir>/spatial-*.png.
//
// node web/tests/spatial.mjs <http base> <out dir>
import { mkdirSync } from "node:fs";
import { chromium } from "playwright-core";

const [base, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const executablePath = process.env.CHROME || "/usr/bin/google-chrome";
const api = async (path, init) => {
  const r = await fetch(base + path, init);
  if (!r.ok) throw new Error(`${path}: HTTP ${r.status} ${await r.text()}`);
  return r.status === 204 ? null : r.json();
};
const send = (method, path, body) => api(path, { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
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
const pass = (what) => console.log(`[spatial.mjs] PASS: ${what}`);
const chain = async (id) => (await api(`/api/v1/strips/${id}`)).chain;

await send("POST", "/api/v1/strips", { id: "music", name: "Music", format: "stereo", patch: [{ input: 1, channel: 1 }, { input: 1, channel: 2 }] });
await send("POST", "/api/v1/buses", { id: "surround", name: "Surround", format: "5.1" });
await send("PUT", "/api/v1/sends/music/main", {});

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
try {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#processing`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  await page.getByLabel("Signal", { exact: true }).selectOption("music");

  // M/S / width from the palette; the width through its pot.
  await page.locator(".palette .pchip", { hasText: /^M\/S \/ Width/ }).click();
  await until("a width node on music", async () => (await chain("music")).some((n) => n.type === "width"));
  await page.getByLabel("Width value").fill("40");
  await page.getByLabel("Width value").press("Tab");
  await until("width 40 %", async () => (await chain("music")).find((n) => n.type === "width").width_pct === 40);
  pass("M/S / width added from the palette, width 40 %");

  // EQ: a band switched to high-pass gets a slope; 24 dB/oct through the segmented control.
  await page.locator(".palette .pchip", { hasText: /^EQ/ }).click();
  await until("an EQ on music", async () => (await chain("music")).some((n) => n.type === "eq"));
  await page.getByLabel("band 1 type").selectOption("high-pass");
  await until("a high-pass band at 12 dB/oct", async () => {
    const b = (await chain("music")).find((n) => n.type === "eq").bands[0];
    return b.type === "high-pass" && b.slope_db_per_oct === 12;
  });
  await page.getByRole("group", { name: "band 1 slope" }).getByRole("button", { name: "24" }).click();
  await until("slope 24", async () => (await chain("music")).find((n) => n.type === "eq").bands[0].slope_db_per_oct === 24);
  await page.getByLabel("band 1 type").selectOption("bell");
  await until("a bell again, without a slope", async () => {
    const b = (await chain("music")).find((n) => n.type === "eq").bands[0];
    return b.type === "bell" && b.slope_db_per_oct === undefined;
  });
  await page.getByLabel("band 1 type").selectOption("low-pass");
  await until("a low-pass band", async () => (await chain("music")).find((n) => n.type === "eq").bands[0].type === "low-pass");
  pass("EQ: high-pass band with 12 then 24 dB/oct, back to a bell, then low-pass");

  // The format selector: 5.1 says beforehand that the width node will be bypassed; confirm (twice).
  await page.getByLabel("Music format").selectOption("5.1");
  await page.locator(".formatselect .note", { hasText: "Bypassed (cannot run on 5.1): width-1" }).waitFor();
  await page.screenshot({ path: `${out}/spatial-format-pick.png`, fullPage: true });
  const change = page.locator(".formatselect button", { hasText: "Change to 5.1" });
  await change.click();
  await page.locator(".formatselect button", { hasText: "Press again" }).click();
  const music = await until("music is 5.1", async () => {
    const s = await api("/api/v1/strips/music");
    return s.format === "5.1" ? s : null;
  });
  if (!music.chain.find((n) => n.type === "width").bypass) throw new Error("width not bypassed");
  if (music.patch.length !== 6 || music.patch[5]?.channel !== 6) throw new Error(`patch ${JSON.stringify(music.patch)}`);
  if (!(await api("/api/v1/sends/music/main"))) throw new Error("send lost");
  await page.locator(".banner.info", { hasText: "width-1 is bypassed" }).waitFor();
  await page.locator(".nodecard .state-tag", { hasText: "not on 5.1" }).waitFor();
  await page.screenshot({ path: `${out}/spatial-format-5.1.png`, fullPage: true });
  pass("format stereo → 5.1 from the Processing view: confirmed, width bypassed and flagged, patch extended, send kept");
  await page.getByLabel("Music format").selectOption("stereo");
  await page.locator(".formatselect button", { hasText: "Change to stereo" }).click();
  await page.locator(".formatselect button", { hasText: "Press again" }).click();
  await until("music stereo again", async () => (await api("/api/v1/strips/music")).format === "stereo");
  await page.locator(".nodecard .state-tag", { hasText: "not on" }).waitFor({ state: "detached" });
  pass("back to stereo: no flag (the node stays bypassed until switched on)");

  // The surround panner on vox (mono): a click on the right of its pad pans it right.
  await page.getByLabel("Signal", { exact: true }).selectOption("vox");
  await page.locator(".palette .pchip", { hasText: /^Surround panner/ }).click();
  await until("a panner on vox", async () => (await chain("vox")).some((n) => n.type === "surround-pan"));
  const pad = page.locator("svg.surroundpad").first();
  await pad.waitFor();
  await pad.scrollIntoViewIfNeeded();
  const box = await pad.boundingBox();
  await pad.click({ position: { x: box.width * 0.95, y: box.height * 0.5 } });
  const az = await until("panned right", async () => {
    const p = (await chain("vox")).find((n) => n.type === "surround-pan");
    if (p.azimuth_deg > 80 && p.azimuth_deg < 100) return p.azimuth_deg;
    throw new Error(`at ${p.azimuth_deg}°`);
  });
  await send("PUT", "/api/v1/sends/vox/surround", {});
  await page.screenshot({ path: `${out}/spatial-panner.png`, fullPage: true });
  pass(`surround panner: added to Vox, pad clicked to ${az}°`);

  // Down-mix: the global one in Settings (a draft until Apply), a bus's own in its settings.
  await page.goto(`${base}/#settings`);
  await page.getByLabel("Down-mix: centre value").fill("-6");
  await page.getByLabel("Down-mix: centre value").press("Tab");
  if ((await api("/api/v1/mix/settings")).downmix.centre_db !== -3) throw new Error("applied before Apply");
  await page.getByLabel("Down-mix: normalise").check();
  await page.getByLabel("Down-mix: apply").click();
  await until("global down-mix", async () => {
    const d = (await api("/api/v1/mix/settings")).downmix;
    return d.centre_db === -6 && d.normalise === true;
  });
  await page.screenshot({ path: `${out}/spatial-settings-downmix.png`, fullPage: true });
  pass("global down-mix: centre −6 dB, normalised, after Apply");
  await page.goto(`${base}/#processing`);
  await page.getByLabel("Signal", { exact: true }).selectOption("main");
  await page.getByLabel("Own down-mix").check();
  await until("main has its own down-mix", async () => (await api("/api/v1/buses/main")).downmix?.centre_db === -6);
  await page.getByLabel("Bus down-mix: LFE value").fill("0");
  await page.getByLabel("Bus down-mix: LFE value").press("Tab");
  await page.getByLabel("Bus down-mix: apply").click();
  await until("main LFE at 0 dB", async () => (await api("/api/v1/buses/main")).downmix?.lfe_db === 0);
  await page.screenshot({ path: `${out}/spatial-bus-downmix.png`, fullPage: true });
  await page.getByLabel("Own down-mix").uncheck();
  await until("main follows the mix again", async () => (await api("/api/v1/buses/main")).downmix === undefined);
  pass("bus down-mix: own (LFE 0 dB), then the mix's again");

  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
} finally {
  await browser.close();
}
