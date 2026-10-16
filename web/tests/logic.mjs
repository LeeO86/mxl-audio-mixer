// Browser check of the logic editor (§11.3) against a running mixer with strips (tests/integration/ui.sh
// sets up the show): the palette (click and drag), cables, live values, undo and redo, copy and paste
// within and between graphs, Delete, zoom, pan and the minimap, and the narrow layout. Every edit is
// checked on the API. Screenshots go to <out dir>.
//
// node web/tests/logic.mjs <http base> <out dir>
import { mkdirSync } from "node:fs";
import { chromium } from "playwright-core";

const [base, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const executablePath = process.env.CHROME || "/usr/bin/google-chrome";
const json = { "content-type": "application/json" };
const api = async (path, init) => {
  const r = await fetch(base + path, init);
  if (!r.ok) throw new Error(`${path}: HTTP ${r.status} ${await r.text()}`);
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
const pass = (what) => console.log(`[logic.mjs] PASS: ${what}`);

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
const made = [];
const muted = new Set();
try {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && console.log(`[logic.mjs] console: ${m.text()}`));
  const mod = process.platform === "darwin" ? "Meta" : "Control";
  const node = (id) => page.locator(`.vue-flow__node[data-id="${id}"]`);
  const graphOf = async (id) => api(`/api/v1/logic/${id}`);
  const center = async (selector) => {
    const b = await page.locator(selector).boundingBox();
    return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
  };
  // A click on an empty part of the canvas: the keys go to the editor, not to a field.
  const focusCanvas = async () => {
    const b = await page.locator(".logiccanvas").boundingBox();
    await page.mouse.click(b.x + b.width / 2, b.y + b.height - 12);
  };

  await page.goto(`${base}/#logic`);
  await page.getByLabel("New logic graph name").fill("Editor");
  await page.getByRole("button", { name: "+ Graph" }).click();
  const g = await until("graph Editor", async () => (await api("/api/v1/logic")).find((x) => x.name === "Editor"));
  made.push(g.id);
  await page.getByLabel("Logic graph", { exact: true }).selectOption(g.id);
  await page.locator(".lpalette").waitFor();

  // The palette: a click adds a Number in the middle of the view; a drag drops Set mute where it lands.
  await page.locator('.lpalette [data-palette="const"]').click();
  await until("const-1 from the palette", async () => (await graphOf(g.id)).blocks.some((b) => b.id === "const-1"));
  const box = await page.locator(".logiccanvas").boundingBox();
  await page.locator('.lpalette [data-palette="set-mute"]').dragTo(page.locator(".logiccanvas"), { targetPosition: { x: box.width - 260, y: 120 } });
  const sm = await until("set-mute-1 dropped", async () => (await graphOf(g.id)).blocks.find((b) => b.id === "set-mute-1"));
  if (!sm.signal) throw new Error(`set-mute-1 without a signal: ${JSON.stringify(sm)}`);
  muted.add(sm.signal);
  await node("set-mute-1").waitFor();
  const dropped = await node("set-mute-1").boundingBox();
  const at = { x: box.x + box.width - 260, y: box.y + 120 };
  if (at.x < dropped.x || at.x > dropped.x + dropped.width || at.y < dropped.y - 10 || at.y > dropped.y + dropped.height + 10) throw new Error(`dropped at ${JSON.stringify(dropped)}, not at ${JSON.stringify(at)}`);
  pass("palette: a click adds a block, a drag drops one where it lands");

  // Value, then a cable: the live value of the action shows "on".
  await page.getByLabel("const-1 value").fill("1");
  await page.getByLabel("const-1 value").press("Enter");
  await until("const is 1", async () => (await graphOf(g.id)).blocks.find((b) => b.id === "const-1").value === 1);
  await page.getByRole("button", { name: "Fit" }).click();
  await page.waitForTimeout(400);
  const from = await center('.vue-flow__node[data-id="const-1"] .vue-flow__handle.source');
  const to = await center('.vue-flow__node[data-id="set-mute-1"] .vue-flow__handle.target');
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(to.x, to.y, { steps: 12 });
  await page.mouse.up();
  await until("the cable", async () => (await graphOf(g.id)).wires.length === 1);
  await page.locator('[data-live="set-mute-1"].on').waitFor({ timeout: 3000 });
  if ((await page.locator('[data-live="const-1"]').textContent()).trim() !== "1") throw new Error("const-1 does not show 1");
  await page.screenshot({ path: `${out}/logic-editor.png` });
  pass("live values on the blocks: the number shows 1, the action shows on");

  // Undo and redo, by key and by button, each one edit.
  await focusCanvas();
  await page.keyboard.press(`${mod}+z`);
  await until("undo: no cable", async () => (await graphOf(g.id)).wires.length === 0);
  await page.keyboard.press(`${mod}+z`);
  await until("undo: const back to 0", async () => (await graphOf(g.id)).blocks.find((b) => b.id === "const-1").value === 0);
  await page.keyboard.press(`${mod}+Shift+z`);
  await until("redo: const 1", async () => (await graphOf(g.id)).blocks.find((b) => b.id === "const-1").value === 1);
  await page.getByRole("button", { name: "Redo" }).click();
  await until("redo: the cable", async () => (await graphOf(g.id)).wires.length === 1);
  if (!(await page.getByRole("button", { name: "Redo" }).isDisabled())) throw new Error("Redo still enabled");
  pass("undo and redo: Ctrl+Z twice, Ctrl+Shift+Z and the Redo button, each one edit");

  // Multi-select, copy, paste: new ids, the cable between them, moved 40 px; one undo removes the paste.
  await node("const-1").click();
  await node("set-mute-1").click({ modifiers: [mod] });
  await until("two selected", async () => (await page.locator(".vue-flow__node.selected").count()) === 2);
  await page.keyboard.press(`${mod}+c`);
  await page.keyboard.press(`${mod}+v`);
  const pastedGraph = await until("pasted", async () => {
    const d = await graphOf(g.id);
    return d.blocks.length === 4 ? d : null;
  });
  const c1 = pastedGraph.blocks.find((b) => b.id === "const-1");
  const c2 = pastedGraph.blocks.find((b) => b.id === "const-2");
  if (!c2 || c2.value !== 1 || c2.x !== c1.x + 40 || c2.y !== c1.y + 40) throw new Error(`pasted const: ${JSON.stringify(c2)}`);
  if (!pastedGraph.wires.some((w) => w.from_block === "const-2" && w.to_block === "set-mute-2")) throw new Error(`pasted wires: ${JSON.stringify(pastedGraph.wires)}`);
  await until("the pasted blocks selected", async () => (await page.locator(".vue-flow__node.selected").count()) === 2 && (await node("const-2").getAttribute("class")).includes("selected"));
  await page.keyboard.press(`${mod}+z`);
  await until("undo the paste in one step", async () => {
    const d = await graphOf(g.id);
    return d.blocks.length === 2 && d.wires.length === 1;
  });
  await page.keyboard.press(`${mod}+Shift+z`);
  await until("redo the paste", async () => (await graphOf(g.id)).blocks.length === 4);
  pass("copy and paste: new ids, the cable between them, 40 px down and right; undo and redo as one step");

  // Delete the two selected pasted blocks with their cable as one edit; undo brings them back.
  await focusCanvas();
  await node("const-2").click();
  await node("set-mute-2").click({ modifiers: [mod] });
  await page.keyboard.press("Delete");
  await until("deleted", async () => {
    const d = await graphOf(g.id);
    return d.blocks.length === 2 && d.wires.length === 1;
  });
  await focusCanvas();
  await page.keyboard.press(`${mod}+z`);
  await until("undo the delete", async () => {
    const d = await graphOf(g.id);
    return d.blocks.length === 4 && d.wires.length === 2;
  });
  pass("Delete removes the selected blocks and their cable as one edit; undo restores them");

  // Paste into another graph.
  await page.getByLabel("New logic graph name").fill("Other");
  await page.getByRole("button", { name: "+ Graph" }).click();
  const other = await until("graph Other", async () => (await api("/api/v1/logic")).find((x) => x.name === "Other"));
  made.push(other.id);
  await page.getByLabel("Logic graph", { exact: true }).selectOption(other.id);
  await page.getByRole("button", { name: "Paste blocks" }).click();
  await until("pasted into Other", async () => {
    const d = await graphOf(other.id);
    return d.blocks.length === 2 && d.wires.length === 1 && d.blocks.some((b) => b.id === "const-1") ? d : null;
  });
  pass("paste into another graph: the blocks and their cable, ids free in that graph");

  // Somebody else changes the graph: undo refuses to overwrite it and clears the history.
  await page.getByLabel("Logic graph", { exact: true }).selectOption(g.id);
  await api(`/api/v1/logic/${g.id}`, { method: "PATCH", headers: json, body: JSON.stringify({ wires: [] }) });
  await until("the other change shown", async () => (await page.locator(".vue-flow__edge").count()) === 0);
  await page.getByRole("button", { name: "Undo" }).click();
  await until("history cleared", async () => page.getByRole("button", { name: "Undo" }).isDisabled());
  if ((await graphOf(g.id)).wires.length !== 0) throw new Error("undo overwrote another client's change");
  pass("undo after another client's change: refused, history cleared");

  // Zoom with the controls and the wheel, pan by dragging, the minimap drawn.
  const transform = () => page.locator(".vue-flow__transformationpane").evaluate((e) => getComputedStyle(e).transform);
  const t0 = await transform();
  await page.locator(".vue-flow__controls-zoomin").click();
  await until("zoomed in", async () => (await transform()) !== t0);
  const t1 = await transform();
  const b2 = await page.locator(".logiccanvas").boundingBox();
  await page.mouse.move(b2.x + b2.width / 2, b2.y + b2.height - 15);
  await page.mouse.down();
  await page.mouse.move(b2.x + b2.width / 2 + 100, b2.y + b2.height - 65, { steps: 8 });
  await page.mouse.up();
  await until("panned", async () => (await transform()) !== t1);
  const t2 = await transform();
  await page.mouse.move(b2.x + b2.width / 2, b2.y + b2.height / 2);
  await page.mouse.wheel(0, 300);
  await until("wheel zoom", async () => (await transform()) !== t2);
  if ((await page.locator(".vue-flow__minimap .vue-flow__minimap-node").count()) !== 4) throw new Error("the minimap does not show the 4 blocks");
  pass("zoom (controls, wheel), pan (drag) and the minimap");

  // Narrow screen: the palette goes above the canvas, no horizontal scroll.
  await page.setViewportSize({ width: 420, height: 900 });
  await page.waitForTimeout(300);
  const pal = await page.locator(".lpalette").boundingBox();
  const can = await page.locator(".logiccanvas").boundingBox();
  if (pal.y + pal.height > can.y + 1) throw new Error("the palette is not above the canvas on a narrow screen");
  const scroll = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  if (scroll > 0) throw new Error(`horizontal scroll of ${scroll} px at 420 px`);
  await page.screenshot({ path: `${out}/logic-narrow.png`, fullPage: true });
  pass("narrow screen: palette above the canvas, no horizontal scroll");

  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
  pass("logic editor");
} catch (e) {
  for (const p of browser.contexts().flatMap((c) => c.pages())) await p.screenshot({ path: `${out}/logic-failed.png` }).catch(() => {});
  throw e;
} finally {
  for (const id of made) await api(`/api/v1/logic/${id}`, { method: "DELETE" }).catch(() => {});
  for (const id of muted) await api(`/api/v1/strips/${id}`, { method: "PATCH", headers: json, body: JSON.stringify({ mute: false }) }).catch(() => {});
  await browser.close();
}
