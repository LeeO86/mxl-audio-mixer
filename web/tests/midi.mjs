// MIDI control of the console (A7): first the message code on its own (web/src/midicodec.js, the typed
// fader level of console.js), then in headless Chrome against a running mixer (tests/integration/ui.sh)
// with a fake Web MIDI (two devices): a Mackie Control (faders, mute, solo, select, V-pot, master, the
// motor fader, LED and display feedback) and a generic device with MIDI learn (learn, use, feedback, kept
// over a reload, export). Without Web MIDI the panel says why. Screenshots go to <out dir>.
//
// node web/tests/midi.mjs <http base> <out dir>
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { chromium } from "playwright-core";
import { parseFaderDb } from "../src/console.js";
import { bendToDb, checkMappings, controlValue, dbToBend, exportMappings, feedback, importMappings, lcd, panRing, parseControl, parseMcu, vpotTicks } from "../src/midicodec.js";

const pass = (what) => console.log(`[midi.mjs] PASS: ${what}`);

// ---- the code on its own ----------------------------------------------------------------
assert.equal(bendToDb(dbToBend(0)), 0);
assert.equal(bendToDb(dbToBend(-10)), -10);
assert.equal(dbToBend(-Infinity), 0);
assert.equal(bendToDb(0), -Infinity);
assert.equal(bendToDb(16383), 15);
assert.equal(vpotTicks(0x03), 3);
assert.equal(vpotTicks(0x41), -1);
assert.equal(panRing(0), 0x16);
assert.equal(panRing(-1), 0x11);
assert.equal(panRing(1), 0x1b);
assert.deepEqual(parseMcu([0xe0, 0x00, 0x40]), { kind: "fader", strip: 0, value: 8192 });
assert.deepEqual(parseMcu([0xe8, 0x7f, 0x7f]), { kind: "fader", strip: 8, value: 16383 });
assert.deepEqual(parseMcu([0x90, 0x12, 0x7f]), { kind: "button", button: "mute", strip: 2, on: true });
assert.deepEqual(parseMcu([0x90, 0x0b, 0x00]), { kind: "button", button: "solo", strip: 3, on: false });
assert.deepEqual(parseMcu([0x90, 0x69, 0x7f]), { kind: "touch", strip: 1, on: true });
assert.deepEqual(parseMcu([0xb0, 0x17, 0x42]), { kind: "vpot", strip: 7, ticks: -2 });
assert.deepEqual(parseMcu([0x90, 0x2f, 0x7f]), { kind: "bank", dir: 1 });
assert.equal(parseMcu([0x90, 0x2f, 0x00]), null);
assert.deepEqual(lcd("Vox", 7), [0xf0, 0, 0, 0x66, 0x14, 0x12, 7, 86, 111, 120, 0xf7]);
const cc = parseControl([0xb1, 7, 127]);
assert.deepEqual(cc.control, { type: "cc", channel: 1, number: 7 });
assert.deepEqual(controlValue("fader", cc), { fader_db: 15 });
assert.deepEqual(controlValue("pan", parseControl([0xb0, 10, 0])), { pan: -1 });
assert.equal(controlValue("mute", parseControl([0x90, 36, 100])), "toggle");
assert.equal(controlValue("mute", parseControl([0x80, 36, 0])), null);
assert.equal(controlValue("mute", parseControl([0xb0, 20, 0])), null);
assert.equal(controlValue("fader", parseControl([0x90, 36, 100])), null);
assert.deepEqual(feedback({ type: "cc", channel: 0, number: 7 }, "fader", 0), [0xb0, 7, 95]);
assert.deepEqual(feedback({ type: "note", channel: 2, number: 36 }, "mute", true), [0x92, 36, 127]);
assert.deepEqual(feedback({ type: "pb", channel: 3, number: 0 }, "pan", 0), [0xe3, 0, 64]);
const maps = [{ device: "nano", control: { type: "cc", channel: 0, number: 7 }, target: { signal: "vox", param: "fader" } }];
assert.deepEqual(importMappings(exportMappings(maps)), { ok: maps, dropped: [] });
const mixed = checkMappings([
  ...maps,
  { control: { type: "cc", channel: 1.5, number: 7 }, target: { signal: "vox", param: "fader" } },
  { control: { type: "cc", channel: 16, number: 7 }, target: { signal: "vox", param: "fader" } },
  { control: { type: "note", channel: 0, number: "36" }, target: { signal: "vox", param: "mute" } },
  { control: { type: "cc", channel: 0, number: 20 }, target: { signal: "vox", param: "toString" } },
  { control: { type: "cc", channel: 0, number: 20 }, target: { signal: "vox", param: "mute" }, latch: true },
]);
assert.equal(mixed.ok.length, 2);
assert.equal(mixed.dropped.length, 4);
assert.equal(mixed.ok[1].latch, true);
// A latching button: its value says on or off.
assert.deepEqual(controlValue("mute", parseControl([0xb0, 20, 127]), true), { on: true });
assert.deepEqual(controlValue("mute", parseControl([0xb0, 20, 0]), true), { on: false });
assert.throws(() => importMappings("nope"), /not a JSON file/);
assert.throws(() => importMappings('{"mappings": [{"control": {"type": "cc", "channel": 0, "number": 7}, "target": {"signal": "vox", "param": "eq"}}]}'), /target/);
assert.equal(parseFaderDb("-20", 15), -20);
assert.equal(parseFaderDb(" −3,5 dB", 15), -3.5);
assert.equal(parseFaderDb("+20", 15), 15);
assert.equal(parseFaderDb("-90", 15), -Infinity);
assert.equal(parseFaderDb("-∞", 15), -Infinity);
assert.equal(parseFaderDb("loud", 15), null);
assert.equal(parseFaderDb("inf", 15), null);
assert.equal(parseFaderDb("+inf", 15), null);
pass("the Mackie Control and MIDI learn messages, the mappings file, typed fader levels");

// ---- in the browser ------------------------------------------------------------------------
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

// A fake Web MIDI: inputs and outputs by name; window.__midi.push(port, bytes) plays a message from a
// device, window.__midi.sent lists what the page sent.
const FAKE = () => {
  const sent = [];
  const input = (name) => ({ name, id: name, type: "input", state: "connected", onmidimessage: null });
  const output = (name) => ({ name, id: name, type: "output", state: "connected", send: (d) => sent.push({ port: name, data: [...d] }) });
  const names = ["X-Touch", "nanoKONTROL"];
  const inputs = new Map(names.map((n) => [n, input(n)]));
  const outputs = new Map(names.map((n) => [n, output(n)]));
  window.__midi = { sent, push: (port, data) => inputs.get(port).onmidimessage?.({ data: new Uint8Array(data) }) };
  Object.defineProperty(navigator, "requestMIDIAccess", { value: async () => ({ inputs, outputs, sysexEnabled: true, onstatechange: null }), configurable: true });
};

const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
try {
  // No Web MIDI: the panel says so.
  const bare = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  await bare.addInitScript(() => Object.defineProperty(navigator, "requestMIDIAccess", { value: undefined, configurable: true }));
  const nomidi = await bare.newPage();
  await nomidi.goto(`${base}/#console`);
  await nomidi.getByRole("button", { name: "MIDI", exact: true }).click();
  await nomidi.getByRole("button", { name: "Connect MIDI" }).click();
  await nomidi.locator('[data-midi-state="unsupported"]').waitFor();
  await nomidi.getByRole("dialog").screenshot({ path: `${out}/midi-unsupported.png` });
  await bare.close();
  pass("without Web MIDI the panel says which browsers have it");

  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  await ctx.addInitScript(FAKE);
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base}/#console`);
  await page.locator('[data-signal="vox"]').waitFor();
  const push = (port, data) => page.evaluate(([p, d]) => window.__midi.push(p, d), [port, data]);
  const sentTo = (port) => page.evaluate((p) => window.__midi.sent.filter((m) => m.port === p).map((m) => m.data), port);
  const clearSent = () => page.evaluate(() => (window.__midi.sent.length = 0));
  const hasSent = (port, data) => until(`${port} gets ${data}`, async () => (await sentTo(port)).some((d) => d.join() === data.join()));

  await page.getByRole("button", { name: "MIDI", exact: true }).click();
  await page.getByRole("button", { name: "Connect MIDI" }).click();
  await page.getByLabel("X-Touch mode").selectOption("mcu");
  await page.getByLabel("nanoKONTROL mode").selectOption("generic");
  // The bank runs over the strips, then the buses.
  const mix = await api("/api/v1/mix");
  const surface = [...mix.strips, ...mix.buses];
  const at = (id) => surface.findIndex((x) => x.id === id);
  if (at("vox") !== 0 || at("gtr") !== 1) throw new Error(`the bank starts ${surface.map((x) => x.id).join(", ")}`);
  await page.locator("[data-midi-bank]", { hasText: `1–${Math.min(8, surface.length)} of ${surface.length}` }).waitFor();
  await page.getByRole("dialog").screenshot({ path: `${out}/midi-panel.png` });

  // Mackie Control: vox and gtr are its first two strips; the first bus (main) is on the master fader.
  await send("PATCH", "/api/v1/strips/vox", { pan: 0 });
  const vox = await api("/api/v1/strips/vox");
  await hasSent("X-Touch", [0xe0, dbToBend(vox.fader_db) & 0x7f, dbToBend(vox.fader_db) >> 7]);
  await until("the display shows the names", async () => (await sentTo("X-Touch")).some((d) => d[0] === 0xf0 && String.fromCharCode(...d.slice(7, -1)).startsWith(surface.slice(0, 8).map((x) => `${x.name.slice(0, 6).padEnd(6)} `).join(""))));
  pass("Mackie Control: motor faders and the names on the display at once");
  const b10 = dbToBend(-10);
  await push("X-Touch", [0xe0, b10 & 0x7f, b10 >> 7]);
  await until("vox at −10 dB from the Mackie fader", async () => (await api("/api/v1/strips/vox")).fader_db === -10);
  await push("X-Touch", [0x90, 0x10, 0x7f]);
  await push("X-Touch", [0x90, 0x10, 0x00]);
  await until("vox muted from the Mackie", async () => (await api("/api/v1/strips/vox")).mute === true);
  await hasSent("X-Touch", [0x90, 0x10, 0x7f]);
  await push("X-Touch", [0x90, 0x10, 0x7f]);
  await until("vox unmuted", async () => (await api("/api/v1/strips/vox")).mute === false);
  await push("X-Touch", [0xb0, 0x10, 0x05]);
  await until("vox panned right by 5 ticks", async () => (await api("/api/v1/strips/vox")).pan === 0.1);
  await hasSent("X-Touch", [0xb0, 0x30, panRing(0.1)]);
  await push("X-Touch", [0x90, 0x20, 0x7f]);
  await until("vox centred by the V-pot push", async () => (await api("/api/v1/strips/vox")).pan === 0);
  await push("X-Touch", [0x90, 0x09, 0x7f]);
  await until("gtr PFL from the Mackie solo", async () => (await api("/api/v1/status")).solo?.gtr === "pfl");
  await hasSent("X-Touch", [0x90, 0x09, 0x7f]);
  await push("X-Touch", [0x90, 0x09, 0x7f]);
  await until("gtr PFL off", async () => !(await api("/api/v1/status")).solo?.gtr);
  await push("X-Touch", [0x90, 0x19, 0x7f]);
  await page.locator('[data-signal="gtr"].selected').waitFor();
  await push("X-Touch", [0xe8, dbToBend(-5) & 0x7f, dbToBend(-5) >> 7]);
  await until("main at −5 dB from the master fader", async () => (await api("/api/v1/buses/main")).fader_db === -5);
  // A change from elsewhere moves the motor fader; not while it is touched.
  await push("X-Touch", [0x90, 0x68, 0x7f]);
  await clearSent();
  await send("PATCH", "/api/v1/strips/vox", { fader_db: -20 });
  await page.waitForTimeout(600);
  if ((await sentTo("X-Touch")).some((d) => d[0] === 0xe0)) throw new Error("a touched fader was moved");
  await push("X-Touch", [0x90, 0x68, 0x00]);
  await hasSent("X-Touch", [0xe0, dbToBend(-20) & 0x7f, dbToBend(-20) >> 7]);
  pass("Mackie Control: fader, mute, V-pot pan and push, solo (PFL), select, master; LEDs, ring and motor fader follow, not while touched");

  // Generic MIDI learn: CC 7 on vox's fader, note 36 on gtr's mute.
  await page.getByRole("button", { name: "Learn…" }).click();
  await page.locator("[data-midi-learn]").waitFor();
  await page.locator('[data-signal="vox"] .vtrack').click();
  await page.locator("[data-midi-learn]", { hasText: "Vox fader picked" }).waitFor();
  const before = (await api("/api/v1/strips/vox")).fader_db;
  if (before !== -20) throw new Error(`the click in learn mode moved the fader (${before})`);
  await page.screenshot({ path: `${out}/midi-learn.png` });
  await push("nanoKONTROL", [0xb0, 7, 64]);
  await page.locator("[data-midi-learn]", { hasText: "Learnt: CC 7 · ch 1 → Vox fader" }).waitFor();
  await page.locator('[data-signal="gtr"] .btn.mute').click();
  if ((await api("/api/v1/strips/gtr")).mute) throw new Error("the click in learn mode muted gtr");
  await push("nanoKONTROL", [0x90, 36, 0x7f]);
  await page.locator("[data-midi-learn]", { hasText: "Note 36 · ch 1 → Guitar mute" }).waitFor();
  await page.getByRole("button", { name: "Done" }).click();
  await push("nanoKONTROL", [0xb0, 7, 127]);
  await until("vox at +15 dB from CC 7", async () => (await api("/api/v1/strips/vox")).fader_db === 15);
  await push("nanoKONTROL", [0x90, 36, 0x7f]);
  await push("nanoKONTROL", [0x80, 36, 0x00]);
  await until("gtr muted by note 36", async () => (await api("/api/v1/strips/gtr")).mute === true);
  await hasSent("nanoKONTROL", [0x90, 36, 127]);
  await send("PATCH", "/api/v1/strips/vox", { fader_db: 0 });
  await hasSent("nanoKONTROL", [0xb0, 7, 95]);
  pass("MIDI learn: CC 7 → vox fader, note 36 → gtr mute (toggles on the press); the LED and the fader value go back");

  // Kept in this browser: after a reload MIDI connects again and the mappings work; a bad stored entry
  // (an edited copy) is dropped.
  await page.evaluate(() => {
    const key = "mxl-audio-mixer.midi.mappings";
    const list = JSON.parse(localStorage.getItem(key));
    localStorage.setItem(key, JSON.stringify([...list, { control: { type: "cc", channel: "x" }, target: {} }]));
  });
  await page.reload();
  await page.locator('[data-signal="vox"]').waitFor();
  await page.waitForTimeout(500);
  await push("nanoKONTROL", [0x90, 36, 0x7f]);
  await until("gtr unmuted by note 36 after a reload", async () => (await api("/api/v1/strips/gtr")).mute === false);
  await page.getByRole("button", { name: "MIDI", exact: true }).click();
  await page.locator("[data-midi-map]").nth(1).waitFor();
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export" }).click();
  const file = await (await download).path();
  const { readFileSync } = await import("node:fs");
  const exported = importMappings(readFileSync(file, "utf8"));
  assert.equal(exported.ok.length, 2);
  assert.equal(exported.dropped.length, 0);
  await page.getByRole("dialog").screenshot({ path: `${out}/midi-mappings.png` });
  // A latching button on gtr's mute: 127 mutes, 0 unmutes.
  await page.getByLabel("Mapping 2 latching").check();
  await push("nanoKONTROL", [0x90, 36, 0x7f]);
  await until("gtr muted by the latching note", async () => (await api("/api/v1/strips/gtr")).mute === true);
  await push("nanoKONTROL", [0x80, 36, 0x00]);
  await until("gtr unmuted when it is let go", async () => (await api("/api/v1/strips/gtr")).mute === false);
  pass("kept over a reload (MIDI connects again by itself, a bad stored entry dropped); export gives both mappings; a latching button follows its value");

  await page.getByRole("button", { name: "Disconnect" }).click();
  if (errors.length) throw new Error(`page errors: ${errors.join("; ")}`);
  pass("no page errors");
} finally {
  await send("PATCH", "/api/v1/strips/vox", { fader_db: 0, mute: false, pan: 0 }).catch(() => {});
  await send("PATCH", "/api/v1/strips/gtr", { mute: false }).catch(() => {});
  await send("PATCH", "/api/v1/buses/main", { fader_db: 0 }).catch(() => {});
  await browser.close();
}
