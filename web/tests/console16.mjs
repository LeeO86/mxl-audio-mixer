// Browser check of the console strip of 1.6.0 (A1–A13) against a running mixer (tests/integration/ui.sh):
// the input dialog, the name at the bottom, the first strip's menu, the format dialog, the L/R pan above
// PFL/AFL, the surround panner in a dialog (mono, and per channel on a 5.1 strip), the auto-mix mark and
// switch, the VCA cap, Select, and the unfolded strip with linked and unlinked members. Every action
// through the UI, checked on the API. Screenshots go to <out dir>.
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
const strip16 = (id) => api(`/api/v1/strips/${id}`);

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
const made = { strips: [], buses: [], vcas: [], automixes: [] };
try {
  const p = (n) => Array.from({ length: n }, (_, c) => ({ input: 1, channel: c + 1 }));
  await send("POST", "/api/v1/buses", { id: "a16-sur", name: "Sur16", format: "5.1" });
  made.buses.push("a16-sur");
  await send("POST", "/api/v1/strips", { id: "a16-st", name: "St16", format: "stereo", patch: p(2) });
  await send("POST", "/api/v1/strips", { id: "a16-51", name: "Bed16", format: "5.1", patch: p(2).concat([null, null, null, null]) });
  await send("POST", "/api/v1/strips", { id: "a16-m", name: "Mic16", format: "mono", patch: p(1) });
  await send("POST", "/api/v1/strips", { id: "a16-m2", name: "Mic16b", format: "mono", patch: p(1) });
  made.strips.push("a16-st", "a16-51", "a16-m", "a16-m2");
  await send("PUT", "/api/v1/sends/a16-m/a16-sur");
  await send("PUT", "/api/v1/sends/a16-51/a16-sur");
  await send("POST", "/api/v1/buses", { id: "a16-sur7", name: "Sur16b", format: "7.1" });
  made.buses.push("a16-sur7");
  await send("PUT", "/api/v1/sends/a16-m/a16-sur7", { block_delay: true });
  await send("POST", "/api/v1/vcas", { id: "a16-vca", name: "V16", fader_db: -6, members: ["a16-st"] });
  made.vcas.push("a16-vca");
  await send("POST", "/api/v1/automixes", { id: "a16-am", name: "AM16", members: ["a16-m", "a16-m2"] });
  made.automixes.push("a16-am");

  const ctx = await browser.newContext({ viewport: { width: 1900, height: 1100 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#console`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  const strip = (id) => page.locator(`[data-signal="${id}"]`);
  await strip("a16-51").waitFor();

  // A1: a wider strip shows channel 1's source only; the dialog has every channel and the Link switch.
  if (await strip("a16-st").locator("select").count()) throw new Error("the stereo strip has a select");
  await strip("a16-st").getByLabel("St16 input", { exact: true }).click();
  const inputs = page.getByRole("dialog", { name: "Input: St16" });
  await inputs.getByLabel("St16 link channels").waitFor();
  if ((await inputs.locator("select").count()) !== 2) throw new Error("not two channels");
  await inputs.getByLabel("St16 link channels").uncheck();
  await inputs.getByLabel("St16 channel 2 input").selectOption("1:1");
  await until("R on 1·1", async () => (await strip16("a16-st")).patch[1]?.channel === 1);
  await inputs.getByLabel("St16 link channels").check();
  await inputs.getByLabel("St16 channel 1 input").selectOption("1:1");
  await until("linked again: R on 1·2", async () => (await strip16("a16-st")).patch[1]?.channel === 2);
  await inputs.screenshot({ path: `${out}/console16-A1-input.png` });
  await page.keyboard.press("Escape");
  if ((await strip("a16-m").locator("select").count()) !== 1) throw new Error("a mono strip picks in the strip");
  pass("A1: the stereo strip shows its first source; the dialog routes every channel, with Link");

  // A2: the name at the very bottom; a long name wraps to two lines, then …, the full name as tooltip.
  const long = "A very long strip name that does not fit at all";
  await send("PATCH", "/api/v1/strips/a16-m2", { name: long });
  const nm = strip("a16-m2").locator(":scope > .sname .nm");
  await until("long name", async () => (await nm.textContent()) === long);
  const fader = await strip("a16-m2").locator(".faderrow").boundingBox();
  const name = await strip("a16-m2").locator(":scope > .sname").boundingBox();
  if (name.y < fader.y + fader.height) throw new Error("the name is not below the fader");
  const lines = await nm.evaluate((el) => ({ h: el.clientHeight, line: parseFloat(getComputedStyle(el).lineHeight), cut: el.scrollHeight > el.clientHeight + 1 }));
  if (lines.h > lines.line * 2.2 || !lines.cut) throw new Error(`name lines ${JSON.stringify(lines)}`);
  if (!(await strip("a16-m2").locator(":scope > .sname").getAttribute("title")).includes(long)) throw new Error("no tooltip");
  await strip("a16-m2").screenshot({ path: `${out}/console16-A2-name.png` });
  pass("A2: the name sits below the fader, two lines then …, with the full name as tooltip");

  // A3: the first strip's menu is not covered at the left edge.
  const first = page.locator(".strips > .strip").first();
  await first.getByRole("button", { name: /menu$/ }).click();
  const menu = first.getByRole("menu");
  await menu.waitFor();
  const mb = await menu.boundingBox();
  if (mb.x < 0) throw new Error(`menu at x ${mb.x}`);
  const onTop = await menu.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return [r.left + 4, r.right - 4].every((x) => el.contains(document.elementFromPoint(x, r.top + r.height / 2)));
  });
  if (!onTop) throw new Error("the menu is covered");
  await page.screenshot({ path: `${out}/console16-A3-menu.png`, clip: { x: 0, y: 0, width: 500, height: 500 } });
  pass("A3: the first strip's menu opens inside the window, nothing covers it");

  // A4: the format dialog: "Change to …" beside Close, the same style.
  await menu.getByRole("menuitem", { name: "Format…" }).click();
  const fmt = page.getByRole("dialog", { name: /^Format: / });
  const fmtSel = fmt.locator("select");
  const now = await fmtSel.inputValue();
  await fmtSel.selectOption(now === "stereo" ? "mono" : "stereo");
  const change = fmt.locator(".actions").getByRole("button", { name: /^Change to / });
  const close = fmt.locator(".actions").getByRole("button", { name: "Close" });
  const [cc, cl] = [await change.getAttribute("class"), await close.getAttribute("class")];
  if (cc !== cl) throw new Error(`classes "${cc}" and "${cl}"`);
  const [cb, lb] = [await change.boundingBox(), await close.boundingBox()];
  if (Math.abs(cb.y - lb.y) > 2 || Math.abs(cb.height - lb.height) > 1) throw new Error("not in one row");
  await fmt.screenshot({ path: `${out}/console16-A4-format.png` });
  await close.click();
  await fmt.waitFor({ state: "detached" });
  pass("A4: Change to … sits beside Close in the format dialog, the same style");

  // A5: the L/R pan right above PFL/AFL, on every format: the 5.1 strip's balance.
  const panBox = await strip("a16-51").locator(".strippan").boundingBox();
  const soloBox = await strip("a16-51").locator(".solo").boundingBox();
  if (soloBox.y - (panBox.y + panBox.height) > 8 || soloBox.y < panBox.y) throw new Error("the pan is not right above PFL/AFL");
  await strip("a16-51").locator(".strippan .minipot", { hasText: "Bal" }).locator("input").fill("0.5");
  await until("5.1 balance 0.5", async () => (await strip16("a16-51")).pan === 0.5);
  if (await strip("a16-51").locator(".surroundpad").count()) throw new Error("a panner picture in the strip");
  pass("A5: L/R pan above PFL/AFL; the 5.1 strip has a balance; no surround picture in the strip");

  // A6: Mic16 sends to a 5.1 bus: the surround button opens the panner.
  await strip("a16-m").getByLabel("Mic16 surround pan").click();
  const sd = page.getByRole("dialog", { name: "Surround pan: Mic16 into 5.1" });
  const pad = sd.locator("svg.surroundpad");
  const box = await pad.boundingBox();
  await pad.click({ position: { x: box.width * 0.95, y: box.height * 0.5 } });
  await until("Mic16 right", async () => {
    const s = (await strip16("a16-m")).surround?.[0];
    return s && s.azimuth_deg > 80 && s.azimuth_deg < 100;
  });
  await sd.getByRole("slider", { name: "Spread" }).fill("40");
  await until("spread 40", async () => (await strip16("a16-m")).surround[0].spread_pct === 40);
  // A block-delay send into a surround bus is not panned, and the dialog says so.
  await sd.locator("[data-unpanned]", { hasText: "Sur16b (block delay" }).waitFor();
  await sd.screenshot({ path: `${out}/console16-A6-surround.png` });
  await sd.getByRole("button", { name: "Restore 1:1" }).click();
  await until("Mic16 1:1", async () => !(await strip16("a16-m")).surround);
  await page.keyboard.press("Escape");
  pass("A6: the surround panner in a dialog from the strip: position and spread; Restore 1:1 removes it");

  // A7: no surround panner block: the API refuses one in a chain.
  const r = await fetch(`${base}/api/v1/strips/a16-m/chain`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ type: "surround-pan", layout: "5.1", azimuth_deg: 0 }) });
  if (r.status !== 400 || !(await r.text()).includes("surround")) throw new Error(`a panner block: HTTP ${r.status}`);
  pass("A7: a surround panner block is refused (400); it is the strip's surround pan");

  // A8: the 5.1 strip: a panner per channel (no LFE); C moved to the right rear; Restore 1:1.
  await strip("a16-51").getByLabel("Bed16 surround pan").click();
  const wd = page.getByRole("dialog", { name: "Surround pan: Bed16 into 5.1" });
  if ((await wd.locator(".surcell").count()) !== 5) throw new Error("not 5 panners");
  if (await wd.locator('[data-channel="LFE"]').count()) throw new Error("an LFE panner");
  const cpad = wd.locator('[data-channel="C"] svg.surroundpad');
  const cb2 = await cpad.boundingBox();
  await cpad.click({ position: { x: cb2.width * 0.9, y: cb2.height * 0.85 } });
  await until("C panned", async () => {
    const s = (await strip16("a16-51")).surround;
    return s?.length === 6 && s[2] && s[2].azimuth_deg > 100 && s.filter(Boolean).length === 1;
  });
  await wd.locator('[data-channel="C"]').getByRole("button", { name: "Bed16 C 1:1" }).waitFor();
  await wd.screenshot({ path: `${out}/console16-A8-per-channel.png` });
  await wd.getByRole("button", { name: "Restore 1:1" }).click();
  await until("Bed16 1:1", async () => !(await strip16("a16-51")).surround);
  await page.keyboard.press("Escape");
  pass("A8: a 5.1 strip has a panner per channel (LFE none); one click restores 1:1");

  // A9: both auto-mix members carry the same colour mark.
  const mark = (id) => strip(id).locator('.ammark[data-automix="a16-am"]');
  await mark("a16-m").waitFor();
  const c1 = await mark("a16-m").evaluate((el) => getComputedStyle(el).backgroundColor);
  const c2 = await mark("a16-m2").evaluate((el) => getComputedStyle(el).backgroundColor);
  if (c1 !== c2 || !c1) throw new Error(`marks ${c1} ${c2}`);
  if (await strip("a16-st").locator(".ammark").count()) throw new Error("a mark on a strip of no auto-mix");
  pass(`A9: the auto-mix members carry its colour mark (${c1})`);

  // A11: a click on the auto-mix meter takes Mic16 out of the sharing (grey), again back in.
  const meter = strip("a16-m").getByLabel("Mic16 auto gain");
  await meter.waitFor({ timeout: 8000 });
  const member = async () => (await api("/api/v1/automixes/a16-am")).members.find((m) => m.signal === "a16-m");
  await meter.click();
  await until("Mic16 out of the sharing", async () => (await member()).bypass === true);
  await until("shown off", async () => (await meter.getAttribute("class")).includes("off"));
  await strip("a16-m").screenshot({ path: `${out}/console16-A11-automix-off.png` });
  await meter.click();
  await until("Mic16 back in", async () => (await member()).bypass === false);
  pass("A11: the auto-mix meter switches the strip out of the sharing (grey) and back");

  // A10: St16 under a VCA at −6 dB: a grey VCA cap at −6 dB.
  const ghost = strip("a16-st").locator(".cap.ghost");
  await until("VCA cap at −6", async () => (await ghost.getAttribute("data-ghost")) === "-6.0");
  await send("PATCH", "/api/v1/vcas/a16-vca", { fader_db: -10 });
  await until("VCA cap at −10", async () => (await ghost.getAttribute("data-ghost")) === "-10.0");
  if ((await ghost.textContent()).trim() !== "VCA") throw new Error("no VCA label");
  if (await strip("a16-m").locator(".cap.ghost").count()) throw new Error("a VCA cap without a VCA");
  pass("A10: a grey VCA cap shows the fader with its VCAs (−6, then −10 dB)");

  // A12: Select beside Mute.
  const sel = strip("a16-m2").getByLabel(`${long} select`);
  const [mute, selb] = [await strip("a16-m2").locator(".btn.mute").boundingBox(), await sel.boundingBox()];
  if (Math.abs(mute.y - selb.y) > 2) throw new Error("Select is not beside Mute");
  await sel.click();
  await strip("a16-m2").and(page.locator(".selected")).waitFor();
  if ((await sel.getAttribute("aria-pressed")) !== "true") throw new Error("not pressed");
  pass("A12: Select beside Mute selects the strip");

  // A13: unfold St16: linked, a member moves the strip; unlinked, an offset; a member mute; clear.
  await strip("a16-st").getByLabel("St16 unfold").click();
  const bar = strip("a16-st").getByLabel("St16 members linked");
  await bar.waitFor();
  if ((await bar.getAttribute("aria-pressed")) !== "true") throw new Error("not linked by default");
  await strip("a16-st").getByLabel("St16 R fader down").click();
  await until("linked: the strip at −0.5", async () => (await strip16("a16-st")).fader_db === -0.5);
  if ((await strip16("a16-st")).members) throw new Error("linked made an offset");
  await bar.click();
  await until("unlinked", async () => (await strip16("a16-st")).unlinked === true);
  await strip("a16-st").getByLabel("St16 R fader down").click();
  await strip("a16-st").getByLabel("St16 R fader down").click();
  await until("R offset −1 dB", async () => {
    const s = await strip16("a16-st");
    return s.fader_db === -0.5 && s.members?.[1]?.fader_db === -1 && !s.members[0].fader_db;
  });
  await strip("a16-st").getByLabel("St16 L mute").click();
  await until("L muted alone", async () => {
    const s = await strip16("a16-st");
    return s.members?.[0]?.mute === true && !s.mute;
  });
  await bar.click();
  await until("linked again, offsets kept", async () => {
    const s = await strip16("a16-st");
    return !s.unlinked && s.members?.[1]?.fader_db === -1;
  });
  await strip("a16-st").getByLabel("St16 R fader up").click();
  await until("linked: the strip moves, R keeps −1", async () => {
    const s = await strip16("a16-st");
    return s.fader_db === 0 && s.members?.[1]?.fader_db === -1;
  });
  // Linked, a member's own mute is still cleared by its button (the strip stays unmuted).
  await strip("a16-st").getByLabel("St16 L mute").click();
  await until("L's own mute cleared", async () => {
    const s = await strip16("a16-st");
    return !s.mute && !s.members?.[0]?.mute && s.members?.[1]?.fader_db === -1;
  });
  await strip("a16-st").screenshot({ path: `${out}/console16-A13-unfolded.png` });
  await strip("a16-st").getByLabel("St16 clear member offsets").click();
  await until("offsets cleared", async () => !(await strip16("a16-st")).members);
  await strip("a16-st").getByLabel("St16 unfold").click();
  await strip("a16-st").locator(".faderrow").waitFor();
  pass("A13: unfolded: linked moves all and keeps offsets, unlinked makes an offset and a member mute");

  await page.screenshot({ path: `${out}/console16.png` });
  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
  pass("no page errors");
} finally {
  for (const id of made.automixes) await fetch(`${base}/api/v1/automixes/${id}`, { method: "DELETE" }).catch(() => {});
  for (const id of made.vcas) await fetch(`${base}/api/v1/vcas/${id}`, { method: "DELETE" }).catch(() => {});
  for (const id of made.strips) await fetch(`${base}/api/v1/strips/${id}`, { method: "DELETE" }).catch(() => {});
  for (const id of made.buses) await fetch(`${base}/api/v1/buses/${id}`, { method: "DELETE" }).catch(() => {});
  await browser.close();
}
