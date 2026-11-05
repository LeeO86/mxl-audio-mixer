// Browser check of 1.6.0 D1 against a running mixer (tests/integration/ui.sh): a bus and a strip's direct out
// are in the strip's input list, chosen there (linked: the other channel follows), an entry that would close a
// feedback loop is greyed out, and the server refuses such a patch with the loop in its message.
// Screenshots go to <out dir>.
//
// node web/tests/inputs16.mjs <http base> <out dir>
import { mkdirSync } from "node:fs";
import { chromium } from "playwright-core";

const [base, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const executablePath = process.env.CHROME || "/usr/bin/google-chrome";
const send = async (method, path, body) => {
  const r = await fetch(base + path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text();
  return { status: r.status, body: text ? JSON.parse(text) : null };
};
const api = async (path) => (await send("GET", path)).body;
async function until(what, fn, ms = 5000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    last = await fn().catch((e) => e.message);
    if (last) return last;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`timed out: ${what} (last: ${JSON.stringify(last)})`);
}
const pass = (what) => console.log(`[inputs16.mjs] PASS: ${what}`);

await send("POST", "/api/v1/buses", { id: "d1-main", name: "D1 Main", format: "stereo" });
await send("POST", "/api/v1/strips", { id: "d1-vox", name: "D1 Vox", format: "stereo" });
await send("POST", "/api/v1/strips", { id: "d1-back", name: "D1 Back", format: "stereo" });
await send("PUT", "/api/v1/sends/d1-vox/d1-main", {});
const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
try {
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" })).newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#console`);
  await page.locator("header .pill", { hasText: /^live$/ }).waitFor();
  const input = (id, name) => page.locator(`[data-signal="${id}"]`).getByLabel(`${name} input`);

  // The bus as input of d1-back, linked: channel 2 follows.
  await input("d1-back", "D1 Back").selectOption({ label: "D1 Main (bus)·1" });
  const patch = await until("d1-back reads d1-main", async () => {
    const p = (await api("/api/v1/strips/d1-back")).patch;
    return p[0]?.signal === "d1-main" && p[1]?.signal === "d1-main" && p[1].channel === 2 ? p : null;
  });
  pass(`a bus as a strip input, linked: ${JSON.stringify(patch)}`);
  await page.screenshot({ path: `${out}/amx16-d1-bus-as-input.png` });

  // d1-vox feeds d1-main: as d1-vox's input that would be a loop, so the entry is greyed out.
  const disabled = await input("d1-vox", "D1 Vox").locator("option", { hasText: "D1 Main (bus)·1" }).isDisabled();
  if (!disabled) throw new Error("the looping entry is selectable");
  const own = await input("d1-vox", "D1 Vox").locator("option", { hasText: "D1 Vox (direct out)·1" }).isDisabled();
  if (!own) throw new Error("the strip's own direct out is selectable");
  pass("entries that would close a loop are greyed out");
  await input("d1-vox", "D1 Vox").focus();
  await page.screenshot({ path: `${out}/amx16-d1-loop-greyed.png` });

  // The server refuses it anyway (a client without the check, the API).
  const r = await send("PATCH", "/api/v1/strips/d1-vox", { patch: [{ signal: "d1-main", channel: 1 }, null] });
  const message = r.body?.error || JSON.stringify(r.body);
  if (r.status !== 400 || !/feedback loop/.test(message) || !message.includes("d1-vox → d1-main → d1-vox")) throw new Error(`refusal ${r.status} ${message}`);
  pass(`refused with: ${message}`);
  // A send that would close one through the patch: refused too.
  const s = await send("PUT", "/api/v1/sends/d1-back/d1-main", {});
  if (s.status !== 400) throw new Error(`send d1-back → d1-main answered ${s.status}`);
  pass(`a send that closes the loop through the patch is refused (${s.status})`);
  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
} finally {
  await browser.close();
  for (const id of ["d1-back", "d1-vox"]) await send("DELETE", `/api/v1/strips/${id}`);
  await send("DELETE", "/api/v1/buses/d1-main");
}
