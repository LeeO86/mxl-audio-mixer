// Browser check of the logic editor (§11.3) against a running mixer with strips (tests/integration/ui.sh
// sets up the show): the palette (click and drag, from the block catalog), cables, live values, undo and
// redo, copy and paste within and between graphs, Delete, zoom, pan and the minimap, typed ports and a
// refused cable, validation markers, macros (group, open, ungroup), dry run, events, and the narrow
// layout. Every edit is checked on the API. Screenshots go to <out dir>.
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
const madeMacros = [];
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
  // The view has stopped moving (after Fit or a zoom): the same transform three times 60 ms apart.
  const settled = async () => {
    let last = "";
    let same = 0;
    await until("the view settled", async () => {
      const t = await page.locator(".vue-flow__transformationpane").evaluate((e) => getComputedStyle(e).transform);
      same = t === last ? same + 1 : 0;
      last = t;
      await new Promise((r) => setTimeout(r, 60));
      return same >= 2;
    });
  };
  const fit = async () => {
    await page.getByRole("button", { name: "Fit" }).click();
    await settled();
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
  if (g.dry_run !== true) throw new Error("a new graph is not in dry run");
  await page.locator("[data-state]", { hasText: "DRY RUN" }).waitFor({ timeout: 3000 });
  pass("a new graph starts in dry run, and the toolbar says so");

  // The palette: a click adds a Number in the middle of the view; a drag drops Set mute where it lands.
  await page.locator('.lpalette [data-palette="const"]').click();
  await until("const-1 from the palette", async () => (await graphOf(g.id)).blocks.some((b) => b.id === "const-1"));
  const box = await page.locator(".logiccanvas").boundingBox();
  await page.locator('.lpalette [data-palette="set-mute"]').dragTo(page.locator(".logiccanvas"), { targetPosition: { x: box.width - 260, y: 120 } });
  // An action gets no target by itself: it is made with "" (inert), marked in amber until one is chosen.
  const blank = await until("set-mute-1 dropped", async () => (await graphOf(g.id)).blocks.find((b) => b.id === "set-mute-1"));
  if (blank.signal !== "") throw new Error(`set-mute-1 got a target by itself: ${blank.signal}`);
  await page.locator('.lnode.warn[data-lnode="set-mute-1"]').waitFor({ timeout: 3000 });
  await page.locator("[data-waiting]").waitFor({ timeout: 3000 });
  await page.getByLabel("set-mute-1 signal", { exact: true }).selectOption("vox");
  const sm = await until("set-mute-1 on vox", async () => {
    const b = (await graphOf(g.id)).blocks.find((x) => x.id === "set-mute-1");
    return b.signal === "vox" ? b : null;
  });
  await until("the warning gone", async () => (await page.locator('.lnode.warn[data-lnode="set-mute-1"]').count()) === 0);
  muted.add(sm.signal);
  await node("set-mute-1").waitFor();
  const dropped = await node("set-mute-1").boundingBox();
  const at = { x: box.x + box.width - 260, y: box.y + 120 };
  if (at.x < dropped.x || at.x > dropped.x + dropped.width || at.y < dropped.y - 10 || at.y > dropped.y + dropped.height + 10) throw new Error(`dropped at ${JSON.stringify(dropped)}, not at ${JSON.stringify(at)}`);
  pass("palette: a click adds a block, a drag drops one where it lands; an action starts without a target, marked, until one is chosen");

  // Arrow keys step through the choices without applying them; Escape puts the old one back, Enter applies.
  const target = page.getByLabel("set-mute-1 signal", { exact: true });
  await target.focus();
  await target.press("ArrowDown");
  await page.waitForTimeout(500);
  if ((await graphOf(g.id)).blocks.find((b) => b.id === "set-mute-1").signal !== "vox") throw new Error("an arrow key moved the action");
  await target.press("Escape");
  if ((await target.inputValue()) !== "vox") throw new Error("Escape did not put vox back");
  await target.press("ArrowDown");
  await target.press("Enter");
  await until("Enter applies", async () => (await graphOf(g.id)).blocks.find((b) => b.id === "set-mute-1").signal !== "vox");
  await target.selectOption("vox");
  await until("back on vox", async () => (await graphOf(g.id)).blocks.find((b) => b.id === "set-mute-1").signal === "vox");
  pass("a target chosen with the arrow keys applies on Enter only; Escape puts it back");

  // Value, then a cable: the live value of the action shows "on".
  // Typed, then the tab redraws a few times (polls, ports, selection) before Enter: the number stays.
  await page.getByLabel("const-1 value").fill("1");
  await page.locator(".lpalette").hover();
  await page.waitForTimeout(800);
  await page.getByLabel("const-1 value").press("Enter");
  await until("const is 1", async () => (await graphOf(g.id)).blocks.find((b) => b.id === "const-1").value === 1);
  await fit();
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
  await until("nothing left to redo", () => page.getByRole("button", { name: "Redo" }).isDisabled());
  pass("undo and redo: Ctrl+Z twice, Ctrl+Shift+Z and the Redo button, each one edit");

  // Ctrl+Z in a text field is the field's own undo, not the graph's.
  await page.getByLabel("New logic graph name").focus();
  await page.keyboard.press(`${mod}+z`);
  await page.waitForTimeout(600);
  if ((await graphOf(g.id)).wires.length !== 1 || (await page.getByRole("button", { name: "Undo" }).isDisabled())) throw new Error("Ctrl+Z in a field undid a graph edit");
  pass("shortcuts: Ctrl+Z in a field leaves the graph alone");

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

  // A webhook's URL is asked for before the block is made.
  await page.locator('.lpalette [data-palette="webhook"]').click();
  await page.getByRole("dialog", { name: "Choose the target" }).waitFor({ timeout: 3000 });
  if (!(await page.getByRole("button", { name: "Add", exact: true }).isDisabled())) throw new Error("Add is enabled before the URL is given");
  await page.getByLabel("new url", { exact: true }).fill("http://127.0.0.1:9/hook");
  await page.getByLabel("new url", { exact: true }).press("Enter");
  await page.getByRole("button", { name: "Add", exact: true }).click();
  await until("the webhook", async () => (await graphOf(other.id)).blocks.some((b) => b.type === "webhook" && b.url === "http://127.0.0.1:9/hook"));
  pass("a webhook asks for its URL first");

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

  // The palette is the server's block catalog.
  const catalog = await api("/api/v1/logic-blocks");
  // Every type but the macro ones, and each macro of the show (it adds an instance).
  const listed = catalog.filter((e) => e.type !== "macro" && !e.type.startsWith("macro-")).length + (await api("/api/v1/logic-macros")).length;
  if ((await page.locator(".lpalette .pitem").count()) !== listed) throw new Error(`palette: ${await page.locator(".lpalette .pitem").count()} items, the catalog lists ${listed}`);
  pass(`palette: the ${listed} block types and macros of GET /api/v1/logic-blocks and /api/v1/logic-macros`);

  // Typed ports: colours, and a cable the input cannot convert is refused while it is drawn.
  const m = await api("/api/v1/logic", {
    method: "POST",
    headers: json,
    body: JSON.stringify({
      name: "Macro",
      blocks: [
        { id: "const-1", type: "const", value: 1, x: 0, y: 0 },
        { id: "not-1", type: "not", x: 260, y: 0 },
        { id: "or-1", type: "or", x: 520, y: 0 },
        { id: "set-mute-1", type: "set-mute", signal: "gtr", x: 780, y: 0 },
        { id: "text-1", type: "text", value: "hello", x: 0, y: 170 },
        { id: "trigger-1", type: "trigger", x: 0, y: 300 },
        { id: "toggle-1", type: "toggle", x: 260, y: 300 },
      ],
      wires: [
        { from_block: "const-1", from_port: "out", to_block: "not-1", to_port: "in" },
        { from_block: "not-1", from_port: "out", to_block: "or-1", to_port: "a" },
        { from_block: "trigger-1", from_port: "out", to_block: "toggle-1", to_port: "in" },
      ],
    }),
  });
  made.push(m.id);
  muted.add("gtr");
  await page.getByLabel("Logic graph", { exact: true }).selectOption(m.id);
  await node("text-1").waitFor();
  await fit();
  const handle = (id, side, port) => `.vue-flow__node[data-id="${id}"] .vue-flow__handle.${side}[data-handleid="${port}"]`;
  const cls = async (sel) => (await page.locator(sel).getAttribute("class")) || "";
  if (!(await cls(handle("const-1", "source", "out"))).includes("t-number")) throw new Error("const-1 out is not a number port");
  if (!(await cls(handle("set-mute-1", "target", "in"))).includes("t-boolean")) throw new Error("set-mute-1 in is not a boolean port");
  if (!(await cls(handle("text-1", "source", "out"))).includes("t-string")) throw new Error("text-1 out is not a string port");
  const drag = async (fromSel, toSel, during) => {
    const a = await center(fromSel);
    const z = await center(toSel);
    await page.mouse.move(a.x, a.y);
    await page.mouse.down();
    await page.mouse.move((a.x + z.x) / 2, (a.y + z.y) / 2, { steps: 8 });
    if (during) await during();
    await page.mouse.move(z.x, z.y, { steps: 8 });
    await page.mouse.up();
  };
  await drag(handle("text-1", "source", "out"), handle("set-mute-1", "target", "in"), async () => {
    if (!(await cls(handle("set-mute-1", "target", "in"))).includes("nope")) throw new Error("the boolean input is not dimmed while a text cable is drawn");
    if ((await cls(handle("or-1", "target", "a"))).includes("nope") !== true) throw new Error("or-1 a (boolean) is not dimmed for a text cable");
  });
  await page.waitForTimeout(600);
  if ((await graphOf(m.id)).wires.length !== 3) throw new Error("a text was wired into a boolean input");
  pass("typed ports: coloured handles; a text cable cannot reach a boolean input, which is dimmed while drawing");

  // A loop without a block that remembers is refused: the issues show as markers and the cable stays dashed.
  await drag(handle("or-1", "source", "out"), handle("not-1", "target", "in"));
  await page.locator(".lissues").waitFor({ timeout: 5000 });
  await until("a marker", async () => (await page.locator(".lnode.bad, .lnode .vue-flow__handle.bad, .vue-flow__edge.bad").count()) > 0);
  await page.locator(".vue-flow__edge.ghost").first().waitFor({ state: "attached", timeout: 3000 });
  if (!(await graphOf(m.id)).wires.some((w) => w.from_block === "const-1")) throw new Error("the refused loop was saved");
  await page.screenshot({ path: `${out}/logic-markers.png` });
  await page.locator(".lissues").getByRole("button", { name: "Dismiss" }).click();
  await until("markers cleared", async () => (await page.locator(".lissues, .vue-flow__edge.ghost, .lnode.bad").count()) === 0);
  pass("validation: a refused loop shows the server's issues on the blocks, ports and the dashed cable; Dismiss clears them");

  // Group not-1 into a macro: its two cables cross the group, so its in and out are exposed.
  await node("not-1").click();
  await page.getByRole("button", { name: "Group into macro" }).click();
  await page.getByLabel("New macro name").fill("Inverter");
  if (!(await page.getByLabel("Expose not-1 in").isChecked()) || !(await page.getByLabel("Expose not-1 in").isDisabled())) throw new Error("not-1 in must be exposed");
  await page.getByRole("button", { name: "Create macro" }).click();
  const grouped = await until("grouped", async () => {
    const d = await graphOf(m.id);
    return d.blocks.some((b) => b.type === "macro") && !d.blocks.some((b) => b.id === "not-1") ? d : null;
  });
  const inst = grouped.blocks.find((b) => b.type === "macro");
  const macro = await api(`/api/v1/logic-macros/${inst.macro}`);
  if (macro.name !== "Inverter" || !macro.blocks.some((b) => b.type === "macro-in" && b.name === "in") || !macro.blocks.some((b) => b.type === "macro-out" && b.name === "out")) throw new Error(`macro: ${JSON.stringify(macro)}`);
  if (!grouped.wires.some((w) => w.from_block === "const-1" && w.to_block === inst.id && w.to_port === "in")) throw new Error(`instance wires: ${JSON.stringify(grouped.wires)}`);
  if (!grouped.wires.some((w) => w.from_block === inst.id && w.from_port === "out" && w.to_block === "or-1")) throw new Error(`instance wires: ${JSON.stringify(grouped.wires)}`);
  await focusCanvas();
  await page.keyboard.press(`${mod}+z`);
  await until("undo the group: not-1 back, the macro gone", async () => (await graphOf(m.id)).blocks.some((b) => b.id === "not-1") && !(await api("/api/v1/logic-macros")).some((x) => x.id === inst.macro));
  // The graph is live and the redo re-makes the macro instance: it asks first.
  await page.keyboard.press(`${mod}+Shift+z`);
  await page.getByRole("alertdialog", { name: "Change a live graph" }).waitFor({ timeout: 3000 });
  await page.getByRole("button", { name: "Do it live" }).click();
  await until("redo the group", async () => (await graphOf(m.id)).blocks.some((b) => b.id === inst.id) && (await api("/api/v1/logic-macros")).some((x) => x.id === inst.macro));
  pass("group into a macro with the crossing ports exposed, as one batch; undo and redo restore graph and macro together");

  // Open it: the breadcrumb, the macro's blocks and the instance's live values.
  await node(inst.id).dblclick();
  await page.locator('.lcrumbs button[aria-current="true"]', { hasText: "Inverter" }).waitFor({ timeout: 3000 });
  await node("not-1").waitFor();
  await page.locator('[data-live="not-1"]', { hasText: "off" }).waitFor({ timeout: 3000 });
  await page.screenshot({ path: `${out}/logic-macro.png` });
  await page.locator(".lcrumbs button", { hasText: "Macro" }).first().click();
  await node(inst.id).waitFor();
  pass("open a macro: breadcrumb, its blocks with the instance's live values, back to the graph");

  // Paste into the macro (it is the definition: every instance gets it), then undo it there.
  await node(inst.id).dblclick();
  await node("not-1").waitFor();
  await node("not-1").click();
  await page.keyboard.press(`${mod}+c`);
  await page.keyboard.press(`${mod}+v`);
  await until("pasted into the macro", async () => (await api(`/api/v1/logic-macros/${inst.macro}`)).blocks.some((b) => b.id === "not-2"));
  await focusCanvas();
  await page.keyboard.press(`${mod}+z`);
  await until("undone in the macro", async () => !(await api(`/api/v1/logic-macros/${inst.macro}`)).blocks.some((b) => b.id === "not-2"));
  // Its own input does not paste into a graph: copy in-in and not-1, paste in the graph.
  await node("in-in").click();
  await node("not-1").click({ modifiers: [mod] });
  await page.keyboard.press(`${mod}+c`);
  await page.locator(".lcrumbs button", { hasText: "Macro" }).first().click();
  await node(inst.id).waitFor();
  await page.getByRole("button", { name: "Paste blocks" }).click();
  const withPaste = await until("pasted into the graph", async () => {
    const d = await graphOf(m.id);
    return d.blocks.some((b) => b.type === "not") ? d : null;
  });
  if (withPaste.blocks.some((b) => b.type === "macro-in")) throw new Error("a macro input was pasted into a graph");
  await focusCanvas();
  await page.keyboard.press(`${mod}+z`);
  await until("the graph paste undone", async () => !(await graphOf(m.id)).blocks.some((b) => b.type === "not"));
  pass("paste into a macro and undo there; a macro's own input is left out when pasting into a graph");

  // Ungroup in a live graph asks first; "switch to dry run first" does that, then ungroups. The blocks
  // come back with the cables; the macro, now unused, goes.
  const liveDialog = page.getByRole("alertdialog", { name: "Change a live graph" });
  const stillGrouped = async () => (await graphOf(m.id)).blocks.some((b) => b.type === "macro");
  await node(inst.id).click();
  await page.getByRole("button", { name: "Ungroup" }).click();
  await liveDialog.waitFor({ timeout: 3000 });
  await page.getByRole("button", { name: "Cancel" }).click();
  await page.waitForTimeout(500);
  if (!(await stillGrouped())) throw new Error("Cancel ungrouped");
  // The switch to dry run fails: nothing else is written.
  await page.route("**/api/v1/logic-batch", (r) => r.fulfill({ status: 500, contentType: "application/json", body: '{"error":"test: refused"}' }), { times: 1 });
  await node(inst.id).click();
  await page.getByRole("button", { name: "Ungroup" }).click();
  await liveDialog.waitFor({ timeout: 3000 });
  await page.getByRole("button", { name: "Switch to dry run first" }).click();
  await page.locator(".banner", { hasText: "Not switched to dry run" }).waitFor({ timeout: 3000 });
  await page.waitForTimeout(500);
  if (!(await stillGrouped()) || (await graphOf(m.id)).dry_run) throw new Error("a failed switch to dry run still changed the graph");
  pass("a live graph: Cancel changes nothing; a failed switch to dry run changes nothing");
  await node(inst.id).click();
  await page.getByRole("button", { name: "Ungroup" }).click();
  await liveDialog.waitFor({ timeout: 3000 });
  await page.getByRole("button", { name: "Switch to dry run first" }).click();
  const flat = await until("ungrouped", async () => {
    const d = await graphOf(m.id);
    return d.blocks.some((b) => b.type === "not") && !d.blocks.some((b) => b.type === "macro") ? d : null;
  });
  const inner = flat.blocks.find((b) => b.type === "not").id;
  if (!flat.wires.some((w) => w.from_block === "const-1" && w.to_block === inner) || !flat.wires.some((w) => w.from_block === inner && w.to_block === "or-1")) throw new Error(`ungrouped wires: ${JSON.stringify(flat.wires)}`);
  if ((await api("/api/v1/logic-macros")).some((x) => x.id === inst.macro)) throw new Error("the unused macro was kept");
  if ((await graphOf(m.id)).dry_run !== true) throw new Error("not switched to dry run");
  await page.locator("[data-state]", { hasText: "DRY RUN" }).waitFor({ timeout: 3000 });
  pass(`ungroup in a live graph: confirmed after switching to dry run; ${inner} back with its cables, the unused macro removed`);

  // Undo never changes on or dry run: the ungroup is undone, the graph stays in dry run.
  await focusCanvas();
  await page.keyboard.press(`${mod}+z`);
  await until("ungroup undone", async () => (await graphOf(m.id)).blocks.some((b) => b.type === "macro"));
  if ((await graphOf(m.id)).dry_run !== true) throw new Error("undo took the graph live");
  await page.keyboard.press(`${mod}+Shift+z`);
  await until("ungroup redone", async () => !(await graphOf(m.id)).blocks.some((b) => b.type === "macro"));
  pass("undo and redo around a dry-run switch leave the graph in dry run");

  // Dry run: the action is listed, not done.
  await until("dry run on", async () => (await graphOf(m.id)).dry_run === true);
  await page.getByLabel("const-1 value", { exact: true }).fill("0");
  await page.getByLabel("const-1 value", { exact: true }).press("Enter");
  await page.getByLabel("Wire from", { exact: true }).selectOption(inner);
  await page.getByLabel("Wire to", { exact: true }).selectOption("set-mute-1");
  await page.getByRole("button", { name: "+ Wire" }).click();
  await page.locator('[data-action="set-mute-1"]', { hasText: "dry run" }).first().waitFor({ timeout: 5000 });
  if ((await api("/api/v1/strips/gtr")).mute) throw new Error("dry run muted Guitar");
  pass("dry run: the action is listed with its dry-run mark, Guitar is not muted");

  // Events: Fire counts on the trigger and flips the toggle.
  await page.getByRole("button", { name: "Fire trigger-1" }).click();
  await page.locator('[data-live="trigger-1"]', { hasText: "×1" }).waitFor({ timeout: 3000 });
  await page.locator('[data-live="toggle-1"]', { hasText: "on" }).waitFor({ timeout: 3000 });
  pass("events: the trigger shows its count, the toggle flipped");

  // Ungrouping past 64 blocks is refused: 63 numbers and a macro of two blocks.
  await api("/api/v1/logic-macros", { method: "POST", headers: json, body: JSON.stringify({ id: "two", name: "Two", blocks: [{ id: "a", type: "not" }, { id: "b", type: "not", x: 200 }] }) });
  madeMacros.push("two");
  const big = await api("/api/v1/logic", {
    method: "POST",
    headers: json,
    body: JSON.stringify({ name: "Big", dry_run: true, blocks: [...Array.from({ length: 63 }, (_, i) => ({ id: `const-${i + 1}`, type: "const", value: i, x: (i % 9) * 200, y: 200 + Math.floor(i / 9) * 100 })), { id: "macro-1", type: "macro", macro: "two", x: 0, y: 0 }] }),
  });
  made.push(big.id);
  await page.getByLabel("Logic graph", { exact: true }).selectOption(big.id);
  await node("macro-1").waitFor();
  await fit();
  await node("macro-1").click();
  await page.getByRole("button", { name: "Ungroup" }).click();
  await page.locator(".banner", { hasText: "at most 64" }).waitFor({ timeout: 3000 });
  if ((await graphOf(big.id)).blocks.length !== 64) throw new Error("the ungroup past 64 blocks was written");
  pass("ungroup past 64 blocks: refused with a message, nothing written");

  // "Do it live": a toggle pasted into a live graph; redo of a paste asks again.
  const lg = await api("/api/v1/logic", { method: "POST", headers: json, body: JSON.stringify({ name: "Live", blocks: [{ id: "toggle-1", type: "toggle", x: 0, y: 0 }] }) });
  made.push(lg.id);
  await page.getByLabel("Logic graph", { exact: true }).selectOption(lg.id);
  await page.locator("[data-state]", { hasText: "LIVE" }).waitFor({ timeout: 3000 });
  await node("toggle-1").click();
  await page.keyboard.press(`${mod}+c`);
  await page.keyboard.press(`${mod}+v`);
  await liveDialog.waitFor({ timeout: 3000 });
  await page.getByRole("button", { name: "Do it live" }).click();
  await until("pasted live", async () => (await graphOf(lg.id)).blocks.some((b) => b.id === "toggle-2"));
  if ((await graphOf(lg.id)).dry_run) throw new Error("Do it live switched to dry run");
  await focusCanvas();
  await page.keyboard.press(`${mod}+z`);
  await until("paste undone", async () => !(await graphOf(lg.id)).blocks.some((b) => b.id === "toggle-2"));
  await page.keyboard.press(`${mod}+Shift+z`);
  await liveDialog.waitFor({ timeout: 3000 });
  await page.getByRole("button", { name: "Cancel" }).click();
  await page.waitForTimeout(500);
  if ((await graphOf(lg.id)).blocks.some((b) => b.id === "toggle-2")) throw new Error("a cancelled redo was written");
  pass("a live graph: Do it live pastes and stays live; a redo that re-makes a block asks again, Cancel keeps it undone");

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
  for (const x of (await api("/api/v1/logic-macros").catch(() => [])) || []) if (x.name === "Inverter") await api(`/api/v1/logic-macros/${x.id}`, { method: "DELETE" }).catch(() => {});
  for (const id of madeMacros) await api(`/api/v1/logic-macros/${id}`, { method: "DELETE" }).catch(() => {});
  for (const id of muted) await api(`/api/v1/strips/${id}`, { method: "PATCH", headers: json, body: JSON.stringify({ mute: false }) }).catch(() => {});
  await browser.close();
}
