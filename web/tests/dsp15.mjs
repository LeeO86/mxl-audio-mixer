// Browser check of the 1.5.0 processing items (B1–B6) against a running mixer (tests/integration/ui.sh sets
// up the show with the tone on input 1): copy and paste of units (B1), every multiband band's gain
// reduction (B2), the leveler (B3), the flow view with its endpoints (B4), the surround panner's spread
// bubble (B5) and the per-send surround panner in the Routing tab (B6). Every action goes through the UI
// and is checked on the API. Screenshots go to <out dir>/dsp15-*.png.
//
// node web/tests/dsp15.mjs <http base> <out dir>
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
const pass = (what) => console.log(`[dsp15.mjs] PASS: ${what}`);
const strip = async (id) => api(`/api/v1/strips/${id}`);
const bus = async (id) => api(`/api/v1/buses/${id}`);

await send("POST", "/api/v1/strips", { id: "dx", name: "Dx", format: "mono", patch: [{ input: 1, channel: 1 }] });
await send("POST", "/api/v1/strips", { id: "dx2", name: "Dx 2", format: "mono", patch: [{ input: 1, channel: 2 }] });
await send("POST", "/api/v1/buses", { id: "stream", name: "Stream", format: "stereo" });
await send("POST", "/api/v1/buses", { id: "s514", name: "S514", format: "5.1.4" });
await send("PUT", "/api/v1/sends/dx/stream", {});
await send("POST", "/api/v1/strips/dx/chain", { type: "eq", bands: [{ type: "bell", freq_hz: 2500, q: 2, gain_db: 4 }] });
await send("POST", "/api/v1/strips/dx/chain", { type: "compressor", threshold_db: -31, ratio: 5, knee_db: 3, attack_ms: 4, release_ms: 120, makeup_db: 2, detector: "peak" });

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
try {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1100 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#processing`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();

  // B1: copy both units of Dx, paste all onto Dx 2; then paste the compressor onto a changed one.
  await page.getByLabel("Signal", { exact: true }).selectOption("dx");
  await page.getByLabel("Copy eq-1").click();
  await page.getByLabel("Copy compressor-1").click();
  await page.getByRole("group", { name: "Clipboard" }).waitFor();
  await page.getByLabel("Signal", { exact: true }).selectOption("dx2");
  await page.getByRole("button", { name: "Paste all" }).click();
  await until("Dx 2 has the EQ and the compressor", async () => {
    const c = (await strip("dx2")).chain;
    return c.length === 2 && c[0].type === "eq" && c[0].bands[0].freq_hz === 2500 && c[1].type === "compressor" && c[1].threshold_db === -31;
  });
  await send("PATCH", "/api/v1/strips/dx2/chain/compressor-1", { threshold_db: -10, sidechain_hpf_hz: 150 });
  await until("Dx 2's compressor changed", async () => (await strip("dx2")).chain[1].threshold_db === -10);
  await page.getByLabel("Paste onto compressor-1").click();
  await until("Dx 2's compressor pasted over", async () => {
    const c = (await strip("dx2")).chain[1];
    return c.id === "compressor-1" && c.threshold_db === -31 && c.ratio === 5 && c.sidechain_hpf_hz == null;
  });
  await page.screenshot({ path: `${out}/dsp15-B1-clipboard.png`, fullPage: true });
  await page.getByRole("button", { name: "Clear", exact: true }).click();
  await page.getByRole("group", { name: "Clipboard" }).waitFor({ state: "detached" });
  pass("B1: two units copied, pasted all onto another strip, one pasted onto a changed unit (its parameters), clipboard cleared");

  // B2: a multiband on Stream with band 2 far below the tone: every band's reduction shows.
  await send("POST", "/api/v1/buses/stream/chain", {
    type: "multiband",
    crossovers_hz: [200, 2000],
    bands: [-60, -60, -60].map((t, i) => ({ threshold_db: i === 1 ? t : 0, ratio: 4, attack_ms: 1, release_ms: 100, makeup_db: 0 })),
  });
  await page.getByLabel("Signal", { exact: true }).selectOption("stream");
  const grs = await until("band reductions in the meters", async () => {
    const m = (await api("/api/v1/meters")).stream?.nodes?.["multiband-1"];
    return m?.band_reduction_db?.length === 3 && m.band_reduction_db[1] > 1 ? m.band_reduction_db : null;
  });
  const bars = page.locator(".bandgr .bgr");
  await until("three band bars", async () => (await bars.count()) === 3);
  await until("band 2 shows its reduction", async () => Number(await bars.nth(1).locator(".num").textContent()) > 1);
  await page.getByLabel("Band 3", { exact: true }).click();
  await until("still every band while band 3 is open", async () => Number(await bars.nth(1).locator(".num").textContent()) > 1);
  await page.locator(".bandgr").first().scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${out}/dsp15-B2-multiband.png`, fullPage: true });
  pass(`B2: every band's gain reduction (${grs.map((g) => g.toFixed(1)).join(", ")} dB), also with another band open`);

  // B3: the leveler from the palette; its gain moves towards the target.
  await page.locator(".palette .pchip", { hasText: /^Leveler/ }).click();
  await until("a leveler on Stream", async () => (await bus("stream")).chain.some((n) => n.type === "leveler"));
  await send("PATCH", "/api/v1/buses/stream/chain/leveler-1", { target_lufs: -10, speed_db_per_s: 10, max_gain_db: 20 });
  const lev = await until("the leveler's gain in the meters", async () => {
    const g = (await api("/api/v1/meters")).stream?.nodes?.["leveler-1"]?.leveler_gain_db;
    return g !== undefined && Math.abs(g) > 0.5 ? g : null;
  }, 10000);
  await page.locator(".levgain").waitFor();
  await page.locator(".levgain").scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${out}/dsp15-B3-leveler.png`, fullPage: true });
  pass(`B3: leveler added from the palette, riding ${lev > 0 ? "+" : ""}${lev} dB`);

  // B4: the flow view of Dx: the send to Stream as an endpoint, dragged after the EQ; the EQ dragged behind
  // the compressor (the send goes with it); a direct out added and dragged to the input.
  await page.getByLabel("Signal", { exact: true }).selectOption("dx");
  const flow = page.locator("[data-flow-view]");
  await flow.scrollIntoViewIfNeeded();
  await flow.locator('[data-endpoint="send:stream"]').waitFor();
  await flow.locator('[data-endpoint="send:stream"]').dragTo(flow.locator('[data-gap="1"]'));
  await until("the send after the EQ", async () => JSON.stringify((await api("/api/v1/sends/dx/stream")).tap) === JSON.stringify({ after: "eq-1" }));
  await flow.locator('[data-flow-node="eq-1"]').dragTo(flow.locator('[data-gap="2"]'));
  await until("EQ after the compressor", async () => (await strip("dx")).chain.map((n) => n.id).join() === "compressor-1,eq-1");
  await flow.locator('[data-gap="2"] [data-endpoint="send:stream"]').waitFor();
  await flow.getByLabel("Add an endpoint").selectOption("output");
  await until("Dx published", async () => (await api("/api/v1/mix")).outputs.some((o) => o.signal === "dx"));
  await flow.locator('[data-endpoint="output"]').dragTo(flow.locator('[data-gap="in"]'));
  await until("the direct out at the input", async () => (await api("/api/v1/mix")).outputs.find((o) => o.signal === "dx")?.tap === "input");
  await page.screenshot({ path: `${out}/dsp15-B4-flow.png`, fullPage: true });
  pass("B4: flow view: send dragged after the EQ, the EQ moved behind the compressor (the send with it), direct out added and taken at the input");

  // B5: spread opens: the bubble moves to the centre and grows; at 100 % it covers every speaker.
  if (!(await strip("vox")).chain.some((n) => n.type === "surround-pan")) await send("POST", "/api/v1/strips/vox/chain", { type: "surround-pan", layout: "5.1", azimuth_deg: 0 });
  await page.getByLabel("Signal", { exact: true }).selectOption("vox");
  const pan = (await strip("vox")).chain.find((n) => n.type === "surround-pan");
  const bubble = page.locator("svg.surroundpad [data-bubble]").first();
  const geo = async () => ({ cx: Number(await bubble.getAttribute("cx")), cy: Number(await bubble.getAttribute("cy")), r: Number(await bubble.getAttribute("r")) });
  await send("PATCH", `/api/v1/strips/vox/chain/${pan.id}`, { azimuth_deg: 90, spread_pct: 0 });
  await until("bubble at the position", async () => (await geo()).cx > 130);
  await send("PATCH", `/api/v1/strips/vox/chain/${pan.id}`, { spread_pct: 50 });
  await until("bubble half way, bigger", async () => {
    const g = await geo();
    return g.cx > 100 && g.cx < 115 && g.r > 30;
  });
  await bubble.scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${out}/dsp15-B5-spread-50.png`, fullPage: true });
  await send("PATCH", `/api/v1/strips/vox/chain/${pan.id}`, { spread_pct: 100 });
  await until("bubble over all speakers", async () => {
    const g = await geo();
    return Math.abs(g.cx - 80) < 0.5 && Math.abs(g.cy - 80) < 0.5 && g.r >= 63;
  });
  await page.screenshot({ path: `${out}/dsp15-B5-spread-100.png`, fullPage: true });
  pass("B5: spread moves the bubble from the position to the centre and grows it over every speaker");

  // B6: a send from mono Dx into the 5.1.4 bus: its editor has a surround panner; a click pans the send.
  await send("PUT", "/api/v1/sends/dx/s514", {});
  await page.goto(`${base}/#routing`);
  await page.getByLabel("Dx to S514").click();
  const sp = page.locator("[data-send-pan]");
  await sp.waitFor();
  const pad = sp.locator("svg.surroundpad");
  const box = await pad.boundingBox();
  await pad.click({ position: { x: box.width * 0.5, y: box.height * 0.95 } });
  const az = await until("the send panned behind", async () => {
    const p = (await api("/api/v1/sends/dx/s514")).pan;
    return p && Math.abs(Math.abs(p.azimuth_deg) - 180) < 10 ? p.azimuth_deg : null;
  });
  await page.screenshot({ path: `${out}/dsp15-B6-send-pan.png`, fullPage: true });
  // Not offered into a stereo bus.
  await page.getByLabel("Dx to Stream").click();
  await page.waitForTimeout(200);
  if (await page.locator("[data-send-pan]").count()) throw new Error("a send panner into a stereo bus");
  await page.getByLabel("Dx to S514").click();
  await page.getByRole("button", { name: "Use the strip's panner" }).click();
  await until("the send's pan removed", async () => (await api("/api/v1/sends/dx/s514")).pan === undefined);
  pass(`B6: per-send surround panner into 5.1.4 (pad click → ${az}°), not into stereo, removed again`);

  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
} finally {
  await browser.close();
}
