// Browser check of the 1.6.0 processing items (C1–C8) against a running mixer (tests/integration/ui.sh
// sets up the show with the tone on input 1): the header (C1), the fixed colours (C2), units after the
// fader (C3), an insert with its send looped back to input 2 and the measured round trip (C4), direct
// outs as blocks (C5), the automixer in the flow (C6), a send's tap dragged in the flow (C7) and the
// down-mix matrix in Settings (C8). Every action goes through the UI and is checked on the API.
// Screenshots go to <out dir>/dsp16-*.png.
//
// node web/tests/dsp16.mjs <http base> <out dir>
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
const pass = (what) => console.log(`[dsp16.mjs] PASS: ${what}`);
const strip = async (id) => api(`/api/v1/strips/${id}`);
const types = async (id) => (await strip(id)).chain.map((n) => n.type).join();

await send("POST", "/api/v1/strips", { id: "px", name: "Px", format: "mono", patch: [{ input: 1, channel: 1 }] });
await send("POST", "/api/v1/buses", { id: "pmain", name: "PMain", format: "stereo" });
await send("POST", "/api/v1/buses", { id: "p714", name: "P714", format: "7.1.4" });
await send("PUT", "/api/v1/sends/px/pmain", {});

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
try {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1100 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#processing`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  await page.getByLabel("Signal", { exact: true }).selectOption("px");

  // C1: "n of m" under the signal select, "Format" beside its dropdown, no key hint at the right.
  const info = page.locator("[data-chain-info]");
  await until("the position under the select", async () => /^\d+ of \d+ · 0 of 32 nodes/.test(await info.textContent()));
  const head = page.locator(".chainhead");
  if ((await head.textContent()).includes("[ and ]")) throw new Error("the key hint is still there");
  const [sel, label] = await Promise.all([page.getByLabel("Signal", { exact: true }).boundingBox(), head.locator(".headformat > label").boundingBox()]);
  const [infoBox, fmt] = await Promise.all([info.boundingBox(), head.locator(".headformat select").boundingBox()]);
  if (!(infoBox.y > sel.y + sel.height - 2)) throw new Error("the info is not under the select");
  if (!(label.x + label.width <= fmt.x + 1 && Math.abs(label.y + label.height / 2 - (fmt.y + fmt.height / 2)) < 12)) throw new Error("Format is not left of its dropdown");
  await head.screenshot({ path: `${out}/dsp16-C1-header.png` });
  pass(`C1: "${(await info.textContent()).trim()}" under the select, Format beside its dropdown, no key hint`);

  // C2: 16 fixed colours, no colour picker.
  if (await page.locator('input[type="color"]').count()) throw new Error("a colour picker is still there");
  const swatches = page.getByRole("radiogroup", { name: "Colour" }).getByRole("radio");
  await until("16 colours and none", async () => (await swatches.count()) === 17);
  await page.getByRole("radio", { name: "Teal" }).click();
  await until("Px is teal", async () => (await strip("px")).colour === "#3f8a8a");
  await page.getByRole("radiogroup", { name: "Colour" }).screenshot({ path: `${out}/dsp16-C2-colours.png` });
  await page.getByRole("radio", { name: "No colour" }).click();
  await until("Px has no colour", async () => !(await strip("px")).colour);
  pass("C2: 16 fixed colours (Teal set, then none), no colour picker");

  // C3: the fader as a block: a unit clicked in goes before it; moved after it, it runs post-fader.
  await page.locator('[data-virtual="fader"]').waitFor();
  await page.locator(".palette .pchip", { hasText: /^Fader/ }).click();
  await until("a fader node", async () => (await types("px")) === "fader");
  await page.locator(".palette .pchip", { hasText: /^EQ/ }).click();
  await until("the EQ before the fader", async () => (await types("px")) === "eq,fader");
  await page.getByLabel("Move fader earlier").click();
  await until("the EQ after the fader", async () => (await types("px")) === "fader,eq");
  await page.locator(".nodes").screenshot({ path: `${out}/dsp16-C3-post-fader.png` });
  pass("C3: fader node placed, an EQ added before it and moved after it (post-fader)");

  // C5: direct outs are blocks: two, the first publishes as the strip, the second as px/out-2.
  await page.locator(".palette .pchip", { hasText: /^Direct out/ }).click();
  await page.locator(".palette .pchip", { hasText: /^Direct out/ }).click();
  await until("two direct outs after the fader", async () => (await types("px")) === "fader,eq,direct-out,direct-out");
  const outs = await until("both published", async () => {
    const o = (await api("/api/v1/outputs")).filter((x) => x.source === "px" && x.kind === "direct-out");
    return o.length === 2 ? o.map((x) => x.signal).sort() : null;
  });
  if (outs.join() !== "px,px/out-2") throw new Error(`direct out keys ${outs}`);
  for (let i = 3; i > 0; i--) {
    await page.getByLabel("Move out-2 earlier").click();
    await until(`out-2 at ${i - 1}`, async () => (await strip("px")).chain[i - 1].id === "out-2");
    await page.locator(".nodecard").nth(i - 1).locator('code:text-is("out-2")').waitFor();
  }
  await page.locator('.nodecard:has(code:text-is("out")) .flownode').screenshot({ path: `${out}/dsp16-C5-direct-out.png` });
  pass(`C5: two direct outs (${outs.join(", ")}), one moved before the fader`);

  // C4: an insert before the fader; its send looped back through input 2 (the return) gives a round trip.
  await page.locator(".palette .pchip", { hasText: /^Insert/ }).click();
  await until("an insert", async () => (await types("px")).includes("insert"));
  const key = "px/insert-1";
  const sendOut = await until("the insert's send published", async () => (await api("/api/v1/outputs")).find((o) => o.signal === key));
  const card = page.locator('.nodecard:has(code:text-is("insert-1"))');
  await card.getByLabel("Return input").selectOption("2");
  await until("return on input 2", async () => (await strip("px")).chain.find((n) => n.id === "insert-1").return_input === 2);
  await card.getByRole("button", { name: "Flow…" }).click();
  const flows = await api("/api/v1/flows");
  const f = flows.find((x) => x.flow_id === sendOut.mxl_flow_id);
  if (!f) throw new Error("the send flow is not listed");
  await card.getByLabel("Return flow").selectOption(`${f.domain_id}|${f.flow_id}`);
  await until("input 2 reads the send", async () => (await api("/api/v1/inputs/2")).flow === sendOut.mxl_flow_id, 8000);
  // A sine repeats every period: white noise first in the chain, so the lag is unique.
  await send("POST", "/api/v1/strips/px/chain", { type: "oscillator", waveform: "white", freq_hz: 1000, level_dbfs: -20, position: 0 });
  const rt = await until("a measured round trip above one block", async () => {
    const o = (await api("/api/v1/outputs")).find((x) => x.signal === key);
    return typeof o?.round_trip_ms === "number" && o.round_trip_ms > 2 ? o.round_trip_ms : null;
  }, 20000);
  await until("the round trip in the block", async () => /\d+\.\d ms/.test(await card.locator("[data-round-trip]").textContent()), 5000);
  const m = await api("/api/v1/meters");
  if (!m[`insert:${key}:send`] || !m[`insert:${key}:return`]) throw new Error("no insert meters");
  await card.scrollIntoViewIfNeeded();
  await card.screenshot({ path: `${out}/dsp16-C4-insert.png` });
  await card.getByRole("button", { name: "Bypass" }).click();
  await until("insert bypassed", async () => (await strip("px")).chain.find((n) => n.id === "insert-1").bypass);
  pass(`C4: insert with send ${key}, return input 2 reading it, round trip ${rt.toFixed(1)} ms, meters out and in, bypass`);

  // C6: in an automix, the automixer shows in the flow; placed after the fader from the palette.
  await send("POST", "/api/v1/automixes", { id: "am16", name: "AM16", members: [{ signal: "px" }] });
  await page.locator('[data-virtual="auto"]').waitFor();
  const flow = page.locator("[data-flow-view]");
  await flow.locator(".stage.virtual", { hasText: "Automixer" }).waitFor();
  await page.locator('[data-virtual="auto"] .nhead').dragTo(page.locator(".dropzone"));
  await until("an automixer node at the end (post-fader)", async () => (await types("px")).endsWith("auto"));
  pass("C6: the automixer shows in the flow and is placed after the fader");

  // C7: the send's tap dragged in the flow: right after the fader, before the automixer.
  await flow.scrollIntoViewIfNeeded();
  const chain = (await strip("px")).chain;
  const gap = chain.findIndex((n) => n.type === "fader") + 1;
  await flow.locator('[data-endpoint="send:pmain"]').dragTo(flow.locator(`[data-gap="${gap}"]`));
  await until("the send after the fader", async () => JSON.stringify((await api("/api/v1/sends/px/pmain")).tap) === JSON.stringify({ after: "fader" }));
  await flow.screenshot({ path: `${out}/dsp16-C7-flow.png` });
  pass("C7: the send's tap set in the flow (after the fader, before the post-fader EQ and the automixer)");

  // C8: the down-mix matrix in Settings: from the widest format (7.1.4), a cell edited and applied.
  await page.goto(`${base}/#settings`);
  const matrix = page.locator("[data-downmix-matrix]");
  await page.getByRole("button", { name: "+ Down-mix setting" }).click();
  const set = await until("a new setting", async () => (await api("/api/v1/mix/settings")).downmixes?.find((d) => d.id !== "reference"));
  await page.getByLabel("Down-mix setting").selectOption(set.id);
  await until("from 7.1.4", async () => (await page.getByLabel("Down-mix from").inputValue()) === "7.1.4");
  await page.getByLabel("Down-mix into").selectOption("stereo");
  await matrix.waitFor();
  if ((await matrix.locator("thead th").count()) !== 13) throw new Error("not 12 source channels");
  const c = matrix.getByLabel("C into L", { exact: true });
  await until("C into L at −3", async () => Math.abs(Number(await c.inputValue()) + 3.01) < 0.02);
  await c.fill("-6");
  await page.getByRole("button", { name: "Apply the matrix" }).click();
  await page.getByRole("button", { name: "Apply the matrix" }).click();
  await until("the edited matrix stored", async () => {
    const d = (await api("/api/v1/mix/settings")).downmixes.find((x) => x.id === set.id);
    return d.matrices?.[0]?.from === "7.1.4" && d.matrices[0].to === "stereo" && Math.abs(d.matrices[0].gains_db[0][2] + 6) < 1e-9;
  });
  await page.getByLabel("Mix down-mix").selectOption(set.id);
  await until("the mix uses it", async () => (await api("/api/v1/mix/settings")).downmix_set === set.id);
  await page.locator(".downmixsets").screenshot({ path: `${out}/dsp16-C8-matrix.png` });
  await page.getByLabel("Mix down-mix").selectOption("reference");
  await until("back to the reference", async () => (await api("/api/v1/mix/settings")).downmix_set === "reference");
  pass("C8: a down-mix setting from downmix.json, its 7.1.4 → stereo matrix edited (C into L −6 dB), used by the mix");

  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
} finally {
  await browser.close();
  for (const p of ["/api/v1/strips/px", "/api/v1/buses/pmain", "/api/v1/buses/p714", "/api/v1/automixes/am16"]) await api(p, { method: "DELETE" }).catch(() => {});
}
