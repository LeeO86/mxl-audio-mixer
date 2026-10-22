// Browser check of the console's 1.4.0 strip handling (C1-C12) against a running mixer
// (tests/integration/ui.sh): several strips at once, the new one selected, rename in place, the gear,
// the linked input patch, phase on a 5.1 strip, "Send to…" and "Add source…", a subgroup, delete, the
// divider. Every action through the UI, checked on the API. Screenshots go to <out dir>.
//
// node web/tests/console.mjs <http base> <out dir>
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
const pass = (what) => console.log(`[console.mjs] PASS: ${what}`);

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
const made = { strips: [], buses: [] };
try {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#console`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  const strip = (id) => page.locator(`[data-signal="${id}"]`);
  const menuItem = async (id, name) => {
    await strip(id).getByRole("button", { name: /menu$/ }).click();
    await strip(id).getByRole("menuitem", { name }).click();
  };

  // C6, C4: three stereo strips at once, numbered; the first is selected.
  const before = new Set((await api("/api/v1/strips")).map((s) => s.id));
  await page.getByRole("button", { name: "+ Strip" }).click();
  await page.locator(".addform input").nth(1).fill("Guest");
  await page.locator(".addform select").selectOption("stereo");
  await page.getByLabel("Number of new strips").fill("3");
  await page.locator(".addform").getByRole("button", { name: "Add" }).click();
  const guests = await until("three new strips", async () => {
    const n = (await api("/api/v1/strips")).filter((s) => !before.has(s.id));
    return n.length === 3 ? n : null;
  });
  made.strips.push(...guests.map((s) => s.id));
  if (guests.map((s) => s.name).join() !== "Guest 1,Guest 2,Guest 3") throw new Error(`names ${guests.map((s) => s.name)}`);
  const [g1, g2] = guests.map((s) => s.id);
  await page.locator(`[data-signal="${g1}"].selected`).waitFor({ timeout: 3000 });
  pass(`+ Strip ×3: ${guests.map((s) => `${s.id} "${s.name}"`).join(", ")}; the first is selected`);

  // C1: rename in place (Enter saves, Escape cancels); the gear opens the processing.
  await strip(g1).locator(".sname").click();
  await page.getByLabel("Guest 1 name").fill("Host");
  await page.getByLabel("Guest 1 name").press("Enter");
  await until("renamed Host", async () => (await api(`/api/v1/strips/${g1}`)).name === "Host");
  await strip(g1).locator(".sname").click();
  await page.getByLabel("Host name").fill("Nope");
  await page.getByLabel("Host name").press("Escape");
  await page.waitForTimeout(300);
  if ((await api(`/api/v1/strips/${g1}`)).name !== "Host") throw new Error("Escape renamed");
  await strip(g1).getByRole("button", { name: "Host processing" }).click();
  await until("processing open", async () => new URL(page.url()).hash === "#processing");
  await page.goto(`${base}/#console`);
  await strip(g1).waitFor();
  pass("rename in place (Enter saves, Escape cancels); the gear opens the processing");

  // C3: a linked stereo patch: input 1 channel 1 → L 1·1, R 1·2; unlinked, channel by channel.
  await page.getByLabel("Host input", { exact: true }).selectOption("1:1");
  await until("linked patch 1·1, 1·2", async () => {
    const p = (await api(`/api/v1/strips/${g1}`)).patch.map((x) => x && `${x.input}:${x.channel}`).join();
    if (p !== "1:1,1:2") throw new Error(`patch ${p}`);
    return true;
  });
  await page.getByLabel("Host link channels").uncheck();
  await page.waitForTimeout(300);
  const kept = (await api(`/api/v1/strips/${g1}`)).patch.map((x) => `${x.input}:${x.channel}`).join();
  if (kept !== "1:1,1:2") throw new Error(`unticking Link changed the patch: ${kept}`);
  await page.getByLabel("Host channel 2 input").selectOption("1:1");
  await until("unlinked: R on 1·1", async () => (await api(`/api/v1/strips/${g1}`)).patch[1]?.channel === 1);
  pass("linked input patch: the first channel chosen, R follows; unlinked per channel");

  // C5: phase on a 5.1 strip: one button, a dialog with a toggle per channel, partial state.
  await send("POST", "/api/v1/strips", { id: "it-surr", name: "Surr", format: "5.1" });
  made.strips.push("it-surr");
  await strip("it-surr").getByRole("button", { name: "Surr polarity" }).click();
  await page.getByRole("dialog", { name: "Polarity: Surr" }).getByRole("button", { name: "Surr LFE polarity" }).click();
  await until("LFE inverted", async () => (await api("/api/v1/strips/it-surr")).polarity.join() === "false,false,false,true,false,false");
  await page.getByRole("dialog").getByRole("button", { name: "Done" }).click();
  await strip("it-surr").locator('.polarity .btn[aria-pressed="mixed"]').waitFor({ timeout: 3000 });
  pass("5.1 polarity: one button, LFE inverted in the dialog, the button shows 1/6");

  // C11: "Send to…" from a strip, "Add source…" on a bus; both at 0 dB.
  await menuItem(g1, "Send to…");
  await page.getByRole("dialog", { name: "Send Host to…" }).locator('[data-pick="main"]').click();
  await until("Host → Main", async () => (await api(`/api/v1/sends/${g1}/main`)).gain_db === 0);
  await menuItem("main", "Add source…");
  await page.getByRole("dialog", { name: "Add a source to Main" }).locator(`[data-pick="${g2}"]`).click();
  await until("Guest 2 → Main", async () => (await api(`/api/v1/sends/${g2}/main`)).gain_db === 0);
  pass("Send to… and Add source…: new sends at 0 dB");

  // C12: a bus made a subgroup: its send at −10 dB becomes 0 dB post-pan, locked on the strip.
  await send("POST", "/api/v1/buses", { id: "it-grp", name: "Grp", format: "stereo" });
  made.buses.push("it-grp");
  await send("PUT", `/api/v1/sends/${g1}/it-grp`, { gain_db: -10 });
  await menuItem("it-grp", "Subgroup…");
  const dialog = page.getByRole("dialog", { name: "Grp: subgroup" });
  await dialog.locator(".warnbox", { hasText: "1 of its 1 sends" }).waitFor();
  await page.screenshot({ path: `${out}/console-subgroup.png` });
  await dialog.getByRole("button", { name: "Make it a subgroup" }).click();
  await until("Grp a subgroup, its send locked", async () => {
    const s = await api(`/api/v1/sends/${g1}/it-grp`);
    return (await api("/api/v1/buses/it-grp")).subgroup === true && s.gain_db === 0 && s.tap === "post-pan";
  });
  await strip(g1).locator('[data-subgroup-send="it-grp"]').waitFor({ timeout: 3000 });
  await strip("it-grp").locator(".fmt .sub").waitFor();
  pass("subgroup: the send at −10 dB now post-pan at 0 dB, shown locked on the strip");

  // C2: the divider gives the buses more room; double-click goes back to automatic.
  const width = async () => (await page.locator(".console .masters").boundingBox()).width;
  const w0 = await width();
  const d = await page.locator(".cdivider").boundingBox();
  await page.mouse.move(d.x + d.width / 2, d.y + 40);
  await page.mouse.down();
  await page.mouse.move(d.x + d.width / 2 - 200, d.y + 40, { steps: 5 });
  await page.mouse.up();
  const w1 = await width();
  if (Math.abs(w1 - w0 - 200) > 4) throw new Error(`buses ${w0} → ${w1} px after a 200 px drag`);
  await page.screenshot({ path: `${out}/console-divider.png` });
  await page.locator(".cdivider").dblclick();
  await until("automatic width again", async () => Math.abs((await width()) - w0) < 4);
  pass(`divider: the buses' side ${Math.round(w0)} → ${Math.round(w1)} px, back on a double-click`);

  // C9: delete with confirmation; its sends go with it.
  await menuItem(g1, "Delete…");
  const del = page.getByRole("dialog", { name: "Delete strip Host?" });
  await del.getByText(/2 sends/).waitFor();
  await page.screenshot({ path: `${out}/console-delete.png` });
  await del.getByRole("button", { name: "Delete Host" }).click();
  await until("Host deleted with its sends", async () => {
    const sends = await api("/api/v1/sends");
    return !(await api("/api/v1/strips")).some((s) => s.id === g1) && !sends.some((s) => s.from === g1) ? true : null;
  });
  made.strips.splice(made.strips.indexOf(g1), 1);
  await strip(g1).waitFor({ state: "detached", timeout: 3000 });
  pass("delete from the strip menu, confirmed: the strip and its sends are gone");

  await page.screenshot({ path: `${out}/console-140.png` });
  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
  pass("no page errors");
} finally {
  for (const id of made.strips) await fetch(`${base}/api/v1/strips/${id}`, { method: "DELETE" }).catch(() => {});
  for (const id of made.buses) await fetch(`${base}/api/v1/buses/${id}`, { method: "DELETE" }).catch(() => {});
  await browser.close();
}
