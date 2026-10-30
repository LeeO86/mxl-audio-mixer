// MIDI messages of the console's control surfaces (A7), without any browser or mixer state, so they can be
// tested on their own: Mackie Control (MCU) faders, buttons, V-pots and their feedback, and the generic
// MIDI learn mappings (what a control is, how its value becomes a mixer value and back, the JSON file).
import { FADER_MAX_DB, dbToPos, posToDb } from "./dsp.js";

// ---- Mackie Control ------------------------------------------------------------------
// The Logic Control / Mackie Control protocol: faders are pitch bend on channels 1–8 (master on 9), the
// buttons notes (press: velocity 127, release: 0), the V-pots relative CCs, the LEDs the same notes back.
export const MCU = {
  STRIPS: 8,
  SOLO: 0x08,
  MUTE: 0x10,
  SELECT: 0x18,
  VPOT_PUSH: 0x20,
  BANK_LEFT: 0x2e,
  BANK_RIGHT: 0x2f,
  CHANNEL_LEFT: 0x30,
  CHANNEL_RIGHT: 0x31,
  TOUCH: 0x68, // + strip; 0x70 the master fader
  VPOT: 0x10, // CC + strip: relative turns
  RING: 0x30, // CC + strip: the LED ring
  MASTER: 8, // the master fader's MIDI channel (0-based)
};

/** A fader position as 14-bit pitch bend (0…16383) from dB (the console's fader law). */
export const dbToBend = (db) => Math.round(dbToPos(db) * 16383);
/** dB (one decimal; −Infinity at the bottom) from 14-bit pitch bend. */
export function bendToDb(v) {
  const d = posToDb(Math.min(16383, Math.max(0, v)) / 16383);
  return Number.isFinite(d) ? Math.min(FADER_MAX_DB, Math.round(d * 10) / 10 || 0) : -Infinity;
}

/** A V-pot turn (relative CC value) as signed ticks: 1…63 clockwise, 65…127 anticlockwise. */
export const vpotTicks = (value) => (value & 0x40 ? -(value & 0x3f) : value & 0x3f);

/** The V-pot ring for a pan −1…+1: boost/cut mode (from the centre), LED 1…11, 6 the centre; 0 is off. */
export function panRing(pan) {
  if (pan === null || pan === undefined) return 0;
  return 0x10 | (1 + Math.round(((Math.max(-1, Math.min(1, pan)) + 1) / 2) * 10));
}

/** The 2 × 56 character LCD: SysEx writing `text` at `offset` (0–55 top row, 56–111 bottom row). */
export function lcd(text, offset, xt = false) {
  const chars = [...String(text)].map((ch) => {
    const c = ch.charCodeAt(0);
    return c >= 0x20 && c < 0x7f ? c : 0x3f;
  });
  return [0xf0, 0x00, 0x00, 0x66, xt ? 0x15 : 0x14, 0x12, offset, ...chars, 0xf7];
}

/** A strip's 7 LCD characters: the text cut or padded to 6, then a space. */
export const lcdCell = (text) => `${String(text).slice(0, 6).padEnd(6)} `;

/** The fader level as 6 LCD characters ("  -6.0", "   -oo"). */
export function lcdDb(db) {
  if (!Number.isFinite(db)) return "   -oo";
  return (db > 0 ? `+${db.toFixed(1)}` : db.toFixed(1)).padStart(6);
}

/**
 * What a Mackie Control message means, or null: { kind: "fader", strip, value } (strip 8 the master),
 * { kind: "touch", strip, on }, { kind: "button", button, strip, on }, { kind: "vpot", strip, ticks },
 * { kind: "bank" | "channel", dir }.
 */
export function parseMcu(data) {
  const [status, a = 0, b = 0] = data;
  const type = status & 0xf0;
  const ch = status & 0x0f;
  if (type === 0xe0) return ch <= MCU.MASTER ? { kind: "fader", strip: ch, value: a | (b << 7) } : null;
  if (type === 0xb0 && a >= MCU.VPOT && a < MCU.VPOT + 8) return { kind: "vpot", strip: a - MCU.VPOT, ticks: vpotTicks(b) };
  if (type !== 0x90 && type !== 0x80) return null;
  const on = type === 0x90 && b > 0;
  if (a >= MCU.TOUCH && a <= MCU.TOUCH + 8) return { kind: "touch", strip: a - MCU.TOUCH, on };
  for (const button of ["SOLO", "MUTE", "SELECT", "VPOT_PUSH"]) {
    if (a >= MCU[button] && a < MCU[button] + 8) return { kind: "button", button: button.toLowerCase(), strip: a - MCU[button], on };
  }
  if (!on) return null;
  if (a === MCU.BANK_LEFT || a === MCU.BANK_RIGHT) return { kind: "bank", dir: a === MCU.BANK_LEFT ? -1 : 1 };
  if (a === MCU.CHANNEL_LEFT || a === MCU.CHANNEL_RIGHT) return { kind: "channel", dir: a === MCU.CHANNEL_LEFT ? -1 : 1 };
  return null;
}

// ---- generic MIDI learn --------------------------------------------------------------
/** The mixer controls a MIDI control can be learnt on: a strip's or bus's. */
export const LEARN_PARAMS = {
  fader: { label: "fader", kind: "level" },
  mute: { label: "mute", kind: "toggle" },
  pfl: { label: "PFL", kind: "toggle" },
  afl: { label: "AFL", kind: "toggle" },
  pan: { label: "pan", kind: "pan" },
};

/** The control a MIDI message comes from: { type: "cc" | "note" | "pb", channel, number }, with its value
 * (0…1) and whether it is a press; null for other messages. A note off (or note on at 0) is a release. */
export function parseControl(data) {
  const [status, a = 0, b = 0] = data;
  const type = status & 0xf0;
  const channel = status & 0x0f;
  if (type === 0xb0) return { control: { type: "cc", channel, number: a }, value: b / 127, press: b > 0 };
  if (type === 0x90 || type === 0x80) {
    const press = type === 0x90 && b > 0;
    return { control: { type: "note", channel, number: a }, value: press ? 1 : 0, press };
  }
  if (type === 0xe0) return { control: { type: "pb", channel, number: 0 }, value: (a | (b << 7)) / 16383, press: true };
  return null;
}

export const controlKey = (c) => `${c.type}:${c.channel}:${c.type === "pb" ? 0 : c.number}`;
export const controlText = (c) => `${{ cc: "CC", note: "Note", pb: "Pitch bend" }[c.type]}${c.type === "pb" ? "" : ` ${c.number}`} · ch ${c.channel + 1}`;

/**
 * What a learnt control does with a message (`parsed` from parseControl) on `param`: { fader_db } or
 * { pan } for continuous controls; for a button "toggle" on its press (the release does nothing), or with
 * `latch` (a latching button: 127 on, 0 off; a note on while held) { on } from its value; or null.
 */
export function controlValue(param, parsed, latch = false) {
  const kind = LEARN_PARAMS[param]?.kind;
  if (kind === "toggle") {
    if (latch) return { on: parsed.value >= 0.5 };
    return parsed.press ? "toggle" : null;
  }
  if (parsed.control.type === "note") return null;
  if (kind === "level") {
    const d = posToDb(parsed.value);
    return { fader_db: Number.isFinite(d) ? Math.min(FADER_MAX_DB, Math.round(d * 10) / 10 || 0) : "-inf" };
  }
  if (kind === "pan") return { pan: Math.round((parsed.value * 2 - 1) * 100) / 100 || 0 };
  return null;
}

/** The message that shows a mixer value on a learnt control (motor fader, LED, ring), or null. */
export function feedback(control, param, value) {
  const kind = LEARN_PARAMS[param]?.kind;
  let v; // 0…1
  if (kind === "toggle") v = value ? 1 : 0;
  else if (kind === "level") v = dbToPos(typeof value === "number" ? value : -Infinity);
  else if (kind === "pan") v = (Math.max(-1, Math.min(1, value || 0)) + 1) / 2;
  else return null;
  if (control.type === "pb") {
    const x = Math.round(v * 16383);
    return [0xe0 | control.channel, x & 0x7f, x >> 7];
  }
  if (control.type === "cc") return [0xb0 | control.channel, control.number, Math.round(v * 127)];
  if (control.type === "note" && kind === "toggle") return [0x90 | control.channel, control.number, v ? 127 : 0];
  return null;
}

/** The mappings file (export): { version: 1, mappings: [...] }. */
export const exportMappings = (mappings) => JSON.stringify({ format: "mxl-audio-mixer-midi", version: 1, mappings }, null, 2);

/**
 * Mappings checked one by one (a file's, or the browser's own): the valid ones, and why the others were
 * dropped. A control is { type: "cc" | "note" | "pb", channel: 0–15, number: 0–127 } (integers), a target a
 * signal id and one of LEARN_PARAMS; `latch` is optional.
 */
export function checkMappings(list) {
  const ok = [];
  const dropped = [];
  const int = (v, lo, hi) => Number.isInteger(v) && v >= lo && v <= hi;
  (Array.isArray(list) ? list : []).forEach((m, i) => {
    const c = m?.control;
    const t = m?.target;
    if (!c || !["cc", "note", "pb"].includes(c.type) || !int(c.channel, 0, 15) || !(c.type === "pb" || int(c.number, 0, 127)))
      dropped.push(`mapping ${i + 1}: the MIDI control is not valid`);
    else if (!t || typeof t.signal !== "string" || !t.signal || !Object.hasOwn(LEARN_PARAMS, t.param)) dropped.push(`mapping ${i + 1}: the target is not valid`);
    else
      ok.push({
        device: typeof m.device === "string" ? m.device : "",
        control: { type: c.type, channel: c.channel, number: c.type === "pb" ? 0 : c.number },
        target: { signal: t.signal, param: t.param },
        ...(m.latch === true && LEARN_PARAMS[t.param].kind === "toggle" ? { latch: true } : {}),
      });
  });
  return { ok, dropped };
}

/** The mappings of a file (import): { ok, dropped } as checkMappings; throws when it has none to read. */
export function importMappings(text) {
  let d;
  try {
    d = JSON.parse(text);
  } catch {
    throw new Error("not a JSON file");
  }
  const list = Array.isArray(d) ? d : d?.mappings;
  if (!Array.isArray(list)) throw new Error("no mappings in the file");
  const r = checkMappings(list);
  if (list.length && !r.ok.length) throw new Error(r.dropped[0]);
  return r;
}
