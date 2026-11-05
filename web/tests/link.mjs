// Unit check of the linked changes of a multi-selection (1.6.0 B1, B11): web/src/link.js, without a
// browser or a mixer. node web/tests/link.mjs
import assert from "node:assert/strict";
import { copyBody, linkNode, linkSignal, matchNode, pickIds } from "../src/link.js";

const strip = (id, over = {}) => ({ id, name: id, fader_db: 0, trim_db: 0, width: 1, pan: 0, mute: false, polarity: [false], chain: [], ...over });
let n = 0;
const check = (what, fn) => (fn(), n++, console.log(`[link.mjs] PASS: ${what}`));

check("a fader move keeps every offset", () => {
  const a = strip("a", { fader_db: -10 });
  assert.deepEqual(linkSignal(a, { fader_db: -7 }, strip("b", { fader_db: -20 })), { fader_db: -17 });
  assert.deepEqual(linkSignal(a, { fader_db: 2 }, strip("b", { fader_db: 10 })), { fader_db: 15 }, "the top stops it");
  assert.deepEqual(linkSignal(a, { fader_db: "-inf" }, strip("b", { fader_db: -20 })), { fader_db: "-inf" }, "to the bottom");
  assert.deepEqual(linkSignal(strip("a", { fader_db: "-inf" }), { fader_db: -10 }, strip("b", { fader_db: "-inf" })), { fader_db: -10 }, "from the bottom together");
});
check("trim and width are relative, mute and polarity the same, pan and name not linked", () => {
  const a = strip("a", { trim_db: 3 });
  assert.deepEqual(linkSignal(a, { trim_db: 6 }, strip("b", { trim_db: -2 })), { trim_db: 1 });
  assert.deepEqual(linkSignal(a, { trim_db: 42 }, strip("b", { trim_db: 40 })), { trim_db: 42 });
  assert.deepEqual(linkSignal(strip("a", { width: 1 }), { width: 1.5 }, strip("b", { width: 0.8 })), { width: 1.3 });
  assert.deepEqual(linkSignal(a, { mute: true }, strip("b", { mute: false })), { mute: true });
  assert.deepEqual(linkSignal(a, { polarity: [true] }, strip("b", { polarity: [false, false] })), { polarity: [true, true] });
  assert.equal(linkSignal(a, { pan: -1 }, strip("b")), null);
  assert.equal(linkSignal(a, { name: "x" }, strip("b")), null);
  assert.deepEqual(linkSignal(a, { trim_db: 6 }, { id: "bus", fader_db: 0, mute: false }), null, "a bus has no trim");
});
check("node parameters: dB relative, frequencies by a factor, on/off the same", () => {
  const src = { id: "comp", type: "compressor", threshold_db: -20, ratio: 3, bypass: false, detector: "rms" };
  const other = { id: "comp", type: "compressor", threshold_db: -30, ratio: 6, bypass: true, detector: "peak" };
  assert.deepEqual(linkNode(src, { threshold_db: -15 }, other), { threshold_db: -25 });
  assert.deepEqual(linkNode(src, { ratio: 6 }, other), { ratio: 12 });
  assert.deepEqual(linkNode(src, { bypass: true }, { ...other, bypass: false }), { bypass: true });
  assert.deepEqual(linkNode(src, { threshold_db: 0 }, other), { threshold_db: -10 });
  assert.deepEqual(linkNode(src, { threshold_db: -100 }, { ...other, threshold_db: -90 }), { threshold_db: -100 }, "clamped to the range");
});
check("EQ bands item by item; the other's band types stay", () => {
  const src = { id: "eq", type: "eq", bands: [{ type: "bell", freq_hz: 1000, q: 1, gain_db: 0 }] };
  const other = { id: "eq", type: "eq", bands: [{ type: "high-shelf", freq_hz: 5000, q: 0.7, gain_db: 2 }, { type: "bell", freq_hz: 300, q: 2, gain_db: -3 }] };
  const out = linkNode(src, { bands: [{ type: "bell", freq_hz: 2000, q: 1, gain_db: 3 }] }, other);
  assert.deepEqual(out.bands[0], { type: "high-shelf", freq_hz: 10000, q: 0.7, gain_db: 5 });
  assert.deepEqual(out.bands[1], other.bands[1]);
  // A band added on the source: the list is the same on all.
  const added = [...src.bands, { type: "bell", freq_hz: 500, q: 1, gain_db: 0 }];
  assert.deepEqual(linkNode(src, { bands: added }, other).bands, added);
});
check("the matching node: the same id, else the same place among its type", () => {
  const a = { chain: [{ id: "eq", type: "eq" }, { id: "c1", type: "compressor" }] };
  assert.equal(matchNode(a, "c1", { chain: [{ id: "c1", type: "compressor" }] }).id, "c1");
  assert.equal(matchNode(a, "c1", { chain: [{ id: "x", type: "compressor" }] }).id, "x");
  assert.equal(matchNode(a, "c1", { chain: [{ id: "c1", type: "gate" }] }), null);
});
check("copy: settings and processing, not the name, patch or format", () => {
  const from = strip("a", { name: "A", fader_db: -5, pan: 0.5, chain: [{ id: "eq", type: "eq", bands: [] }], colour: "#ff0000", patch: [{ input: 1, channel: 1 }], format: "mono", polarity: [true] });
  const body = copyBody(from, strip("b", { polarity: [false, false] }));
  assert.equal(body.fader_db, -5);
  assert.equal(body.pan, 0.5);
  assert.equal(body.colour, "#ff0000");
  assert.deepEqual(body.chain, from.chain);
  assert.notEqual(body.chain, from.chain, "a copy");
  assert.deepEqual(body.polarity, [true, true]);
  for (const k of ["name", "patch", "format", "id"]) assert.ok(!(k in body), k);
  const toBus = copyBody({ id: "m", fader_db: 0, mute: false, chain: [], compensate: false }, { id: "n", fader_db: 0, mute: false, chain: [], compensate: true });
  assert.equal(toBus.compensate, false);
  assert.ok(!("trim_db" in toBus));
  assert.equal(toBus.colour, "", "no colour clears the other's");
});
check("click, Ctrl and Shift selection", () => {
  const order = ["a", "b", "c", "d"];
  assert.deepEqual(pickIds(["a"], "a", "c", {}, order), { ids: ["c"], anchor: "c" });
  assert.deepEqual(pickIds(["a"], "a", "c", { ctrl: true }, order).ids, ["a", "c"]);
  assert.deepEqual(pickIds(["a", "c"], "c", "c", { ctrl: true }, order).ids, ["a"]);
  assert.deepEqual(pickIds(["b"], "b", "d", { shift: true }, order).ids, ["b", "c", "d"]);
  assert.deepEqual(pickIds(["d"], "d", "b", { shift: true }, order).ids, ["b", "c", "d"]);
});
console.log(`[link.mjs] ${n} checks passed`);
