// Browser check of the 1.5.0 graph and routing items (C1–C7) against a running mixer started with
// virtual outputs (tests/integration/ui.sh: MIXER_OUTPUTS with "PGM" 5.1 and "Clean" stereo; vox, gtr, main
// and aux from ui.mjs; music and surround from spatial.mjs). Every action goes through the UI and is
// checked on the API. Screenshots go to <out dir>/graph-*.png.
//
// node web/tests/graph.mjs <http base> <out dir>
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
const pass = (what) => console.log(`[graph.mjs] PASS: ${what}`);
const vout = async (n) => api(`/api/v1/virtual-outputs/${n}`);

/** Draw a wire from the source handle of node `from` to the target handle of node `to`. */
async function wire(page, from, to) {
  const a = await page.locator(`.vue-flow__node:has([data-gnode="${from}"]) .vue-flow__handle.source`).boundingBox();
  const b = await page.locator(`.vue-flow__node:has([data-gnode="${to}"]) .vue-flow__handle.target`).boundingBox();
  await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
  await page.mouse.down();
  await page.mouse.move((a.x + b.x) / 2, (a.y + b.y) / 2, { steps: 5 });
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 5 });
  await page.mouse.up();
}

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
try {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));

  // C7 in Routing: the virtual outputs with a source select.
  await page.goto(`${base}/#routing`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  await page.locator('[data-vouts] [data-vout="1"]').waitFor();
  await page.getByLabel("Source of PGM").selectOption(JSON.stringify({ signal: "main" }));
  await until("PGM fed by main", async () => (await vout(1)).source?.signal === "main");
  await page.screenshot({ path: `${out}/graph-c7-routing.png`, fullPage: true });
  pass("Routing: PGM takes the main bus from its source select");

  // C7 in the Graph: both outputs (virtual and published) are output nodes with a wire.
  await page.goto(`${base}/#graph`);
  await page.locator('[data-gnode="_vout_1"]').waitFor();
  await page.locator('[data-gnode="_out_main"]').waitFor();
  await until("PGM wire", async () => (await page.locator('.vue-flow__edge[data-id="vsrc:1"]').count()) === 1);
  await page.getByRole("button", { name: "Fit" }).click();
  await page.waitForTimeout(300);
  await wire(page, "vox", "_vout_2");
  await until("Clean fed by vox", async () => (await vout(2)).source?.signal === "vox");
  await page.locator("[data-vout-edit]").waitFor();
  await page.screenshot({ path: `${out}/graph-c7-wire.png` });
  pass("Graph: a wire from vox into Clean feeds it; its panel opens");
  // An input straight into a virtual output (pass-through, D4's route).
  await wire(page, "_in_1", "_vout_2");
  await until("Clean fed by input 1", async () => (await vout(2)).source?.input === 1);
  await page.locator("[data-vout-edit]").getByRole("button", { name: "Silence" }).click();
  await until("Clean silent", async () => (await vout(2)).source === null);
  pass("Graph: input 1 straight into Clean, then silenced from its panel");
  // C6: a control-room monitor into a virtual output.
  const monitors = await api("/api/v1/monitors");
  const mon = monitors[0]?.id;
  if (mon) {
    await wire(page, `_mon_${mon}`, "_vout_2");
    await until("Clean fed by the monitor", async () => (await vout(2)).source?.monitor === mon);
    pass(`Graph: monitor ${mon} into Clean (C6)`);
  }

  // C1: a mono strip's post-pan send into a stereo bus edits its pan; a 5.1 send into stereo its down-mix.
  // Its own mono strip and post-pan send (the earlier scripts change the others).
  await send("POST", "/api/v1/strips", { id: "c1", name: "C1 mono", format: "mono", patch: [{ input: 1, channel: 1 }] });
  await send("PUT", "/api/v1/sends/c1/main", { tap: "post-pan" });
  // A click on the edge itself (a forced click at its box's centre may hit another wire).
  const edgeClick = (id) => page.locator(`.vue-flow__edge[data-id="${id}"]`).dispatchEvent("click");
  await until("c1 → main edge", async () => (await page.locator('.vue-flow__edge[data-id="send:c1:main"]').count()) === 1);
  await edgeClick("send:c1:main");
  await page.locator("[data-send-edit] [data-strip-pan]").waitFor();
  await page.screenshot({ path: `${out}/graph-c1-pan.png` });
  pass("Graph: the c1 → main send shows the strip's pan");
  const buses = (await api("/api/v1/mix")).buses.map((b) => b.id);
  if (buses.includes("surround")) {
    await send("PUT", "/api/v1/sends/surround/main", {}).catch(() => {});
    await until("surround → main edge", async () => (await page.locator('.vue-flow__edge[data-id="send:surround:main"]').count()) === 1);
    await edgeClick("send:surround:main");
    await page.locator("[data-send-edit] [data-send-downmix]").waitFor();
    await page.locator("[data-send-downmix]").getByRole("button", { name: "Own down-mix" }).click();
    await until("main has its own down-mix", async () => !!(await api("/api/v1/buses/main")).downmix);
    await page.screenshot({ path: `${out}/graph-c1-downmix.png` });
    await send("PATCH", "/api/v1/buses/main", { downmix: null });
    await api("/api/v1/sends/surround/main", { method: "DELETE" });
    pass("Graph: the 5.1 → stereo send edits the down-mix into main");
  }

  // C2 and C3: click an input for its alias and flow.
  await page.locator('[data-gnode="_in_1"]').click();
  await page.locator("[data-input-edit]").waitFor();
  await until("flows listed", async () => (await page.getByLabel("MXL flow").locator("option").count()) > 1);
  await until("connected label shown", async () => (await page.locator("[data-input-edit]").textContent()).includes("Test tone"));
  await page.getByLabel("Alias").fill("Tone in");
  await page.getByLabel("Alias").press("Enter");
  await until("alias saved", async () => (await api("/api/v1/inputs/1")).label === "Tone in");
  await page.locator('[data-gnode="_in_1"]', { hasText: "Tone in" }).waitFor();
  await page.screenshot({ path: `${out}/graph-c2-c3-input.png` });
  // The flow select connects the input again (the same flow: a no-op route through IS-05).
  await page.locator("[data-input-edit]").getByRole("button", { name: "Connect", exact: true }).click();
  await until("input 1 running", async () => (await api("/api/v1/inputs/1")).state === "running");
  await send("PATCH", "/api/v1/inputs/1", { label: "Input 1" });
  pass("Graph: input 1 shows Test tone, takes an alias, connects its flow");

  // C5: a zoom stays over a quick tab switch.
  await page.locator(".vue-flow__pane").hover();
  await page.mouse.wheel(0, -400);
  await page.waitForTimeout(300);
  const before = await page.locator(".vue-flow__transformationpane").getAttribute("style");
  await page.locator("nav button", { hasText: "Console" }).click();
  await page.locator("nav button", { hasText: "Graph" }).click();
  const after = await page.locator(".vue-flow__transformationpane").getAttribute("style");
  if (before !== after) throw new Error(`the graph view changed over a tab switch: ${before} → ${after}`);
  pass("Graph: zoom and pan stay over a tab switch");

  // C4: double-click a strip for its processing, a monitor for the control room.
  await page.getByRole("button", { name: "Fit" }).click();
  await page.waitForTimeout(300);
  await page.locator('[data-gnode="c1"]').dblclick();
  await until("processing tab", async () => (await page.locator('nav button[aria-current="page"]').textContent()).trim() === "Processing");
  const sel = await page.getByLabel("Signal", { exact: true }).inputValue();
  if (sel !== "c1") throw new Error(`processing shows ${sel}, not c1`);
  pass("Graph: double-click on a strip opens its processing");
  if (mon) {
    await page.locator("nav button", { hasText: "Graph" }).click();
    await page.locator(`[data-gnode="_mon_${mon}"]`).dblclick();
    await until("control room tab", async () => (await page.locator('nav button[aria-current="page"]').textContent()).trim() === "Control room");
    pass("Graph: double-click on a monitor opens the control room");
  }

  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
} finally {
  await browser.close();
}
