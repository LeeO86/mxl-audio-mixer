// Browser check of the console's 1.6.0 items B1–B11 against a running mixer (tests/integration/ui.sh,
// strips vox and gtr, buses main and aux): the view and bank beside the tabs, the multi-selection with
// linked (relative) faders, the same mute and PFL on all, linked processing, moving strips and buses, the
// Inputs and Buses views and a custom view, the strip and bus menus as dialogs (meters, + bus, auto-mix,
// MIDI learn, auto, operations, copy). Every action through the UI, checked on the API. Screenshots go to
// <out dir> (amx16-B<n>-*.png).
//
// node web/tests/console16.mjs <http base> <out dir>
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
const pass = (what) => console.log(`[console16.mjs] PASS: ${what}`);
const db = (v) => (v === "-inf" ? -Infinity : v);

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
const before = { strips: (await api("/api/v1/strips")).map((s) => s.id), buses: (await api("/api/v1/buses")).map((b) => b.id), layouts: await api("/api/v1/layouts") };
const hpf = { id: "hpf", type: "filter", mode: "high-pass", freq_hz: 80, slope_db_per_oct: 24 };
let amId = "";
try {
  for (const id of ["vox", "gtr"]) {
    await send("PATCH", `/api/v1/strips/${id}`, { fader_db: id === "vox" ? -10 : -20, mute: false });
    await send("POST", `/api/v1/strips/${id}/chain`, { ...hpf, freq_hz: id === "vox" ? 80 : 160 });
  }
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#console`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  const strip = (id) => page.locator(`[data-signal="${id}"]`);
  await strip("gtr").waitFor();
  const menu = async (id, item) => {
    await strip(id).getByRole("button", { name: /menu$/ }).click();
    if (item) await page.getByRole("menuitem", { name: item }).click();
  };

  // B4: the view and bank beside the tabs, right of them.
  const tools = page.locator("nav #tabtools");
  await tools.getByRole("group", { name: "Console view" }).waitFor();
  const tabsBox = await page.locator("nav button", { hasText: "Settings" }).boundingBox();
  const toolsBox = await tools.boundingBox();
  if (!(toolsBox.x > tabsBox.x + tabsBox.width) || Math.abs(toolsBox.y - tabsBox.y) > 30) throw new Error(`tab tools at ${JSON.stringify(toolsBox)}, tabs end ${JSON.stringify(tabsBox)}`);
  await page.screenshot({ path: `${out}/amx16-B4-tabtools.png`, clip: { x: 0, y: 0, width: 1600, height: 220 } });
  pass("B4: the view and bank sit beside the tabs, right aligned");

  // B1: a click selects, Ctrl adds one, Shift a range; the toolbar says how many.
  await strip("vox").locator(".sfoot").click();
  await page.locator('[data-signal="vox"].selected').waitFor();
  await strip("gtr").locator(".sfoot").click({ modifiers: ["Control"] });
  await page.locator('[data-signal="gtr"].selected').waitFor();
  await page.locator("[data-selection]", { hasText: "2 selected" }).waitFor();
  await strip("gtr").locator(".sfoot").click();
  if (await page.locator('[data-signal="vox"].selected').count()) throw new Error("a plain click kept vox selected");
  await strip("vox").locator(".sfoot").click({ modifiers: ["Shift"] });
  await page.locator('[data-signal="vox"].selected').waitFor();
  const order = (await api("/api/v1/strips")).map((s) => s.id);
  const range = order.slice(Math.min(order.indexOf("vox"), order.indexOf("gtr")), Math.max(order.indexOf("vox"), order.indexOf("gtr")) + 1);
  await until("the Shift range selected", async () => (await page.locator(".strips .strip.selected").count()) === range.length);
  // Only vox and gtr from here on.
  await strip("vox").locator(".sfoot").click();
  await strip("gtr").locator(".sfoot").click({ modifiers: ["Control"] });
  pass(`B1: click, Ctrl+click, Shift+click (range ${range.join(", ")})`);

  // B1: linked fader: relative, one request per step.
  const batches = [];
  page.on("request", (r) => r.url().endsWith("/api/v1/signals") && batches.push(r.method()));
  await strip("vox").locator(".vtrack").focus();
  for (let i = 0; i < 4; i++) await page.keyboard.press("ArrowUp");
  await until("vox −8, gtr −18", async () => {
    const [v, g] = await Promise.all([api("/api/v1/strips/vox"), api("/api/v1/strips/gtr")]);
    return db(v.fader_db) === -8 && db(g.fader_db) === -18;
  });
  if (!batches.length || batches.some((m) => m !== "PATCH")) throw new Error(`batches ${batches}`);
  // To the bottom and back: the offset is lost only at the end of the travel.
  await page.keyboard.press("End");
  await until("both at −inf", async () => (await api("/api/v1/strips/gtr")).fader_db === "-inf" && (await api("/api/v1/strips/vox")).fader_db === "-inf");
  await page.keyboard.press("Home");
  await until("both at 0 from the bottom", async () => db((await api("/api/v1/strips/gtr")).fader_db) === 0 && db((await api("/api/v1/strips/vox")).fader_db) === 0);
  pass(`B1: a fader move of two selected keeps the offset (vox −10 → −8, gtr −20 → −18) in ${batches.length} PATCH /api/v1/signals`);

  // Mute and PFL: the same on all.
  await strip("vox").locator(".btn.mute").click();
  await until("both muted", async () => (await api("/api/v1/strips/vox")).mute && (await api("/api/v1/strips/gtr")).mute);
  await strip("vox").getByRole("button", { name: "PFL Vox" }).click();
  await until("both PFL", async () => {
    const s = (await api("/api/v1/status")).solo || {};
    return s.vox === "pfl" && s.gtr === "pfl";
  });
  await page.screenshot({ path: `${out}/amx16-B1-selection.png` });
  await strip("vox").getByRole("button", { name: "PFL Vox" }).click();
  await until("PFL off on both", async () => {
    const s = (await api("/api/v1/status")).solo || {};
    return !s.vox && !s.gtr;
  });
  await strip("gtr").locator(".btn.mute").click();
  await until("both unmuted", async () => !(await api("/api/v1/strips/vox")).mute && !(await api("/api/v1/strips/gtr")).mute);
  pass("B1: mute and PFL set the same on both selected");

  // Linked processing: the bypass on both, a frequency by the same factor.
  await strip("vox").getByRole("button", { name: "Vox processing" }).click();
  await page.locator("[data-linked]").waitFor();
  const hpfCard = page.locator(".nodecard").filter({ has: page.locator("code", { hasText: /^hpf$/ }) });
  await hpfCard.getByRole("button", { name: "Bypass" }).click();
  let chains = null;
  await until("hpf bypassed on both", async () => {
    const [v, g] = await Promise.all([api("/api/v1/strips/vox/chain"), api("/api/v1/strips/gtr/chain")]);
    chains = { vox: v.map((n) => `${n.id}:${n.type}:${n.bypass}`), gtr: g.map((n) => `${n.id}:${n.type}:${n.bypass}`) };
    return v.find((n) => n.id === "hpf")?.bypass && g.find((n) => n.id === "hpf")?.bypass;
  }).catch((e) => {
    throw new Error(`${e.message} ${JSON.stringify(chains)}`);
  });
  await page.screenshot({ path: `${out}/amx16-B1-processing.png` });
  await hpfCard.getByRole("button", { name: "Bypass" }).click();
  await until("hpf on again on both", async () => !(await api("/api/v1/strips/vox/chain/hpf")).bypass && !(await api("/api/v1/strips/gtr/chain/hpf")).bypass);
  pass("B1: a processing view action (bypass) runs on both selected");
  await page.locator("nav button", { hasText: "Console" }).click();
  await strip("gtr").waitFor();

  // B10, B11: the menu is a dialog; copy vox's settings to the selected gtr (not its name).
  await send("PATCH", "/api/v1/strips/vox", { trim_db: 6, fader_db: -4 });
  await menu("vox");
  await page.locator("[data-strip-menu]").waitFor();
  await page.screenshot({ path: `${out}/amx16-B10-menu.png` });
  await page.getByRole("menuitem", { name: "Copy settings…" }).click();
  await page.screenshot({ path: `${out}/amx16-B11-copy.png` });
  await page.locator("[data-copy-selected]").click();
  await until("gtr has vox's settings", async () => {
    const g = await api("/api/v1/strips/gtr");
    return g.trim_db === 6 && db(g.fader_db) === -4 && g.name === "Guitar" && g.chain.find((n) => n.id === "hpf")?.freq_hz === 80;
  });
  pass("B10, B11: the strip menu is a dialog over the mixer; Copy settings to the selected gives gtr vox's trim, fader and processing, not its name");

  // B7: auto-mix from the menu: both selected join, then none.
  amId = (await send("POST", "/api/v1/automixes", { name: "Panel 16" })).id;
  await menu("vox", "Auto-mix…");
  await page.getByRole("button", { name: "Panel 16" }).click();
  await until("vox and gtr in the auto-mix", async () => (await api(`/api/v1/automixes/${amId}`)).members.map((m) => m.signal).sort().join() === "gtr,vox");
  await menu("gtr", "Auto-mix…");
  await page.screenshot({ path: `${out}/amx16-B7-automix.png` });
  await page.getByRole("button", { name: "None" }).click();
  await until("none in the auto-mix", async () => (await api(`/api/v1/automixes/${amId}`)).members.length === 0);
  await menu("vox", "Auto-mix…");
  await page.getByRole("button", { name: "Add auto-mix…" }).click();
  await page.getByRole("dialog", { name: "Auto-mixes (gain sharing)" }).waitFor();
  await page.keyboard.press("Escape");
  pass("B7: Auto-mix in the strip menu: the two selected join and leave; Add auto-mix opens the auto-mix dialog");

  // B5: the meter scale from the menu.
  await menu("main");
  await page.getByLabel("Meter scale").selectOption("ebu");
  await page.keyboard.press("Escape");
  await until("the scale stored", async () => (await page.evaluate(() => localStorage.getItem("mxl-audio-mixer.meter.scale"))) === "ebu");
  await menu("main");
  await page.getByLabel("Meter scale").selectOption("dbfs");
  await page.keyboard.press("Escape");
  pass("B5: Meters in the bus menu sets the scale");

  // B8: MIDI learn from the menu picks the strip's fader (one strip: off while several are selected).
  await menu("vox");
  if (!(await page.getByRole("menuitem", { name: "MIDI learn" }).isDisabled())) throw new Error("MIDI learn on with two selected");
  await page.keyboard.press("Escape");
  await strip("vox").locator(".sfoot").click();
  await menu("vox", "MIDI learn");
  await page.locator("[data-midi-learn]", { hasText: "Vox" }).waitFor({ timeout: 3000 });
  await page.screenshot({ path: `${out}/amx16-B8-midilearn.png` });
  await page.locator("[data-midi-learn]").getByRole("button", { name: "Done" }).click();
  pass("B8: MIDI learn in the strip menu: learn mode with the strip's fader picked; off while several are selected");

  // B9: Auto and Operations from the menu; B6: + Bus from the bus menu.
  await strip("gtr").locator(".sfoot").click();
  await menu("gtr", "Auto…");
  await page.getByRole("dialog", { name: "Auto-level, ducking and voice detection" }).waitFor();
  await page.keyboard.press("Escape");
  await menu("gtr", "Operations…");
  await page.getByRole("dialog", { name: "Operations" }).waitFor();
  await page.keyboard.press("Escape");
  await menu("main", "+ Bus…");
  await page.getByRole("dialog", { name: "New bus" }).locator("input").nth(0).fill("b16");
  await page.getByRole("dialog", { name: "New bus" }).getByRole("button", { name: "Add", exact: true }).click();
  await until("bus b16", async () => (await api("/api/v1/buses")).some((b) => b.id === "b16"));
  pass("B6, B9: + Bus in the bus menu, Auto and Operations in the menus (dialogs)");

  // B2: move a bus and a strip by its grip (keys and a drag): the show's order.
  await page.locator('[data-slot="b16"] .grip').focus();
  await page.keyboard.press("ArrowLeft");
  await until("b16 before the last bus", async () => {
    const ids = (await api("/api/v1/buses")).map((b) => b.id);
    return ids.indexOf("b16") === ids.length - 2;
  });
  const gripOf = await page.locator('[data-slot="gtr"] .grip').boundingBox();
  const voxBox = await page.locator('[data-slot="vox"]').boundingBox();
  await page.mouse.move(gripOf.x + gripOf.width / 2, gripOf.y + gripOf.height / 2);
  await page.mouse.down();
  await page.mouse.move(voxBox.x + voxBox.width / 2, voxBox.y + 200, { steps: 8 });
  await page.screenshot({ path: `${out}/amx16-B2-drag.png` });
  await page.mouse.up();
  await until("gtr before vox", async () => {
    const ids = (await api("/api/v1/strips")).map((s) => s.id);
    return ids.indexOf("gtr") === ids.indexOf("vox") - 1;
  });
  await until("the console shows gtr before vox", async () => {
    const shownOrder = await page.locator(".strips [data-signal]").evaluateAll((els) => els.map((e) => e.dataset.signal));
    return shownOrder.indexOf("gtr") < shownOrder.indexOf("vox");
  });
  pass("B2: a bus moved by its grip with the arrow keys, a strip dragged before another (the show's order, shown at once)");

  // B3: the Inputs and Buses views, and a custom view with a strip, a bus and a VCA.
  await tools.getByRole("button", { name: "Inputs" }).click();
  await until("only strips", async () => (await page.locator(".busesec").count()) === 0 && (await page.locator('.strips [data-signal="vox"]').count()) === 1);
  await tools.getByRole("button", { name: "Buses" }).click();
  await until("only buses", async () => (await page.locator('.strips [data-signal="main"]').count()) === 1 && (await page.locator('.strips [data-signal="vox"]').count()) === 0);
  await page.screenshot({ path: `${out}/amx16-B3-buses-view.png` });
  await send("POST", "/api/v1/vcas", { id: "v16", name: "VCA 16", members: ["vox"] });
  await tools.getByRole("button", { name: "Edit views…" }).click();
  const editor = page.getByRole("dialog", { name: "Views" });
  await editor.getByRole("button", { name: "+ View" }).click();
  await editor.getByLabel("View name").fill("Talk");
  await editor.locator('[data-put="main"]').click();
  await editor.locator('[data-put="v16"]').click();
  await editor.locator('[data-put="vox"]').click();
  await editor.getByRole("button", { name: "+ Bank" }).click();
  await editor.locator('[data-put="gtr"]').click();
  await page.screenshot({ path: `${out}/amx16-B3-view-editor.png` });
  await editor.getByRole("button", { name: "Save views" }).click();
  await until("view Talk saved", async () => {
    const l = (await api("/api/v1/layouts")).find((x) => x.name === "Talk");
    return l && l.pages[0].signals.join() === "main,v16,vox" && l.pages[1].signals.join() === "gtr";
  });
  await until("the Talk view shown", async () => (await page.locator(".strips > .slot").evaluateAll((els) => els.map((e) => e.dataset.slot))).join() === "main,v16,vox");
  await page.screenshot({ path: `${out}/amx16-B3-custom-view.png` });
  // In a custom view a drag changes the view, not the show's order.
  await page.locator('[data-slot="vox"] .grip').focus();
  await page.keyboard.press("ArrowLeft");
  await until("vox before the VCA in the view", async () => (await api("/api/v1/layouts")).find((x) => x.name === "Talk").pages[0].signals.join() === "main,vox,v16");
  await tools.getByRole("button", { name: "All strips" }).click();
  pass("B3: Inputs and Buses views; a custom view (a bus, a VCA, a strip; a second bank) made in the view editor, saved in the show; moving in it changes the view");

  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
  await ctx.close();
} finally {
  await browser.close();
  // Back as it was: the order, the nodes, the auto-mix, the VCA, the bus, the layouts.
  for (const id of ["vox", "gtr"]) await fetch(`${base}/api/v1/strips/${id}/chain/hpf`, { method: "DELETE" });
  await send("PATCH", "/api/v1/strips/gtr", { trim_db: 0, fader_db: 0 }).catch(() => {});
  await send("PATCH", "/api/v1/strips/vox", { trim_db: 0, fader_db: 0 }).catch(() => {});
  if (amId) await fetch(`${base}/api/v1/automixes/${amId}`, { method: "DELETE" });
  await fetch(`${base}/api/v1/vcas/v16`, { method: "DELETE" });
  await fetch(`${base}/api/v1/buses/b16`, { method: "DELETE" });
  await send("PUT", "/api/v1/layouts", before.layouts).catch(() => {});
  for (const [i, id] of before.strips.entries()) await send("POST", `/api/v1/strips/${id}/move`, { position: i }).catch(() => {});
  for (const [i, id] of before.buses.entries()) await send("POST", `/api/v1/buses/${id}/move`, { position: i }).catch(() => {});
}
