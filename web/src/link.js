// Linked changes of a multi-selection (1.6.0 B1), as pure functions: a change on one selected strip is
// carried to the others relatively (each keeps its offset), except for on/off values, which are set to the
// same value on all. Pan and the label are never linked. The console sends the result as one request
// (PATCH /api/v1/signals).
import { NODE_TYPES } from "./nodes.js";

/** The bottom of the fader travel: below it a linked level is −∞ (−∞ counts as this for the offset). */
export const FADER_FLOOR_DB = -80;
export const FADER_MAX_DB = 15;
export const TRIM_DB = 42;

// Signal fields: relative (dB or a factor's range) or the same value on all. Anything else (name, pan,
// patch, format, chain, tally, afv, subgroup, minus, downmix) stays on the strip that was changed.
const RELATIVE = {
  fader_db: { min: FADER_FLOOR_DB, max: FADER_MAX_DB, floor: true },
  trim_db: { min: -TRIM_DB, max: TRIM_DB },
  width: { min: 0, max: 2 },
};
const SAME = new Set(["mute", "polarity", "loudness", "colour", "recall_safe", "solo_safe", "mute_safe", "compensate"]);

const dbNum = (v) => (v === "-inf" || v === null || v === undefined ? -Infinity : Number(v));
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const round = (v, digits = 2) => Math.round(v * 10 ** digits) / 10 ** digits;

/** Whether a signal field is linked at all. */
export const linksField = (key) => key in RELATIVE || SAME.has(key);

/**
 * The part of `body` (a change of signal `src`) that signal `other` takes: relative fields keep their
 * offset, on/off fields the same value. Returns null when nothing is linked. `from` ({ src, other }: the
 * relative fields of both at the start of the move) keeps the offset over the end of the travel.
 */
export function linkSignal(src, body, other, from = null) {
  const out = {};
  for (const [key, value] of Object.entries(body)) {
    const r = RELATIVE[key];
    if (r) {
      if (!(key in other)) continue; // a bus has no trim or width
      const a = from && key in from.src ? from.src[key] : src[key];
      const b = from && key in from.other ? from.other[key] : other[key];
      if (r.floor) {
        const start = Math.max(r.min, dbNum(a));
        const to = Math.max(r.min, dbNum(value));
        const mine = Math.max(r.min, dbNum(b));
        const v = round(Math.min(r.max, mine + (to - start)), 1);
        out[key] = v <= r.min ? "-inf" : v;
      } else {
        out[key] = round(clamp(Number(b) + (Number(value) - Number(a)), r.min, r.max));
      }
    } else if (SAME.has(key)) {
      if (key === "polarity") {
        if (!Array.isArray(other.polarity)) continue;
        out.polarity = other.polarity.length === value.length ? [...value] : other.polarity.map(() => value.every(Boolean));
      } else if (key === "compensate") {
        if (key in other) out[key] = value;
      } else out[key] = value;
    }
  }
  return Object.keys(out).length ? out : null;
}

// Node parameters: the ranges of the server's validation (src/mix/chain.rs) for the ones in bands and
// lists; the rest come from the node types (nodes.js). Logarithmic ones move by a factor.
const LOG = new Set(["freq_hz", "q", "ratio", "attack_ms", "release_ms", "hold_ms", "lookahead_ms", "sidechain_hpf_hz", "crossover_hz", "crossovers_hz"]);
const RANGES = {
  freq_hz: [10, 20000],
  q: [0.1, 40],
  gain_db: [-24, 24],
  threshold_db: [-100, 0],
  ratio: [1, 1000],
  attack_ms: [0, 1000],
  release_ms: [0, 10000],
  makeup_db: [-24, 24],
  crossovers_hz: [20, 20000],
};
function rangeOf(type, key) {
  const f = NODE_TYPES[type]?.fields?.find((x) => x.key === key);
  if (f && Number.isFinite(f.min) && Number.isFinite(f.max)) return [f.min, f.max];
  return RANGES[key] || [-Infinity, Infinity];
}
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

function relate(type, key, from, to, mine) {
  if (same(from, to)) return mine; // unchanged here: the other keeps its own
  if (typeof to === "number" && typeof from === "number" && typeof mine === "number") {
    const [lo, hi] = rangeOf(type, key);
    const v = LOG.has(key) && from > 0 && mine > 0 ? mine * (to / from) : mine + (to - from);
    return round(clamp(v, lo, hi), 3);
  }
  if (Array.isArray(to) && Array.isArray(from) && Array.isArray(mine)) {
    // A list of the same length (bands, crossovers): item by item, as far as the other has items.
    if (to.length === from.length) return mine.map((m, i) => (i < to.length ? relate(type, key, from[i], to[i], m) : m));
    // One item added or taken out (an EQ band): only that one, at its place, on the other.
    const at = (long, short) => {
      let i = 0;
      while (i < short.length && same(long[i], short[i])) i++;
      return long.slice(i + 1).every((x, k) => same(x, short[i + k])) ? i : -1;
    };
    if (to.length === from.length + 1) {
      const i = at(to, from);
      const pos = i === from.length ? mine.length : Math.min(i, mine.length); // appended: at the other's end
      if (i >= 0) return [...mine.slice(0, pos), to[i], ...mine.slice(pos)];
    }
    if (to.length === from.length - 1) {
      const i = at(from, to);
      if (i >= 0) return i < mine.length ? mine.filter((_, k) => k !== i) : mine;
    }
  }
  if (to && from && mine && typeof to === "object" && typeof from === "object" && typeof mine === "object" && !Array.isArray(to)) {
    const out = { ...mine };
    for (const k of Object.keys(to)) out[k] = relate(type, k, from[k], to[k], mine[k]);
    return out;
  }
  return to; // a new structure, an on/off or a choice: the same on all
}

/** The merge patch that node `other` takes from `body`, a change of node `src` (same type). */
export function linkNode(src, body, other) {
  const out = {};
  for (const [key, value] of Object.entries(body)) {
    if (key === "ramp_ms" || key === "id") continue;
    const v = relate(src.type, key, src[key], value, other[key]);
    if (!same(v, other[key])) out[key] = v;
  }
  return Object.keys(out).length ? out : null;
}

/** The node of signal `other` that matches node `nodeId` of `src`: the same id and type, else the one at
 * the same place among the nodes of its type. */
export function matchNode(src, nodeId, other) {
  const node = src.chain.find((n) => n.id === nodeId);
  if (!node) return null;
  const byId = other.chain.find((n) => n.id === nodeId && n.type === node.type);
  if (byId) return byId;
  const nth = src.chain.filter((n) => n.type === node.type).indexOf(node);
  return other.chain.filter((n) => n.type === node.type)[nth] || null;
}

/** What a copy of settings (B11) gives `to` from `from`: every setting and the processing, not the label,
 * the id, the input patch, the format or the sends. */
export function copyBody(from, to) {
  const keys = ["fader_db", "mute", "chain", "loudness", "colour", "recall_safe", "solo_safe", "mute_safe"];
  if ("trim_db" in to) keys.push("trim_db", "pan", "width");
  if ("compensate" in to) keys.push("compensate");
  const out = {};
  for (const k of keys) if (k in from) out[k] = JSON.parse(JSON.stringify(from[k]));
  if (out.colour === null || out.colour === undefined) out.colour = "";
  if (Array.isArray(from.polarity) && Array.isArray(to.polarity)) out.polarity = to.polarity.length === from.polarity.length ? [...from.polarity] : to.polarity.map(() => from.polarity.every(Boolean));
  // A strip's surround panner and member offsets (A6–A8, A13) where they fit the target's format:
  // one position between mono and stereo strips, else the same format; else the target's go.
  if ("trim_db" in to) {
    const narrow = (f) => f === "mono" || f === "stereo";
    const fits = from.format === to.format || (narrow(from.format) && narrow(to.format));
    out.surround = fits ? JSON.parse(JSON.stringify(from.surround || [])) : [];
    const same = Array.isArray(to.polarity) && to.polarity.length === from.polarity?.length;
    out.members = same ? JSON.parse(JSON.stringify(from.members || [])) : [];
    out.unlinked = same ? !!from.unlinked : false;
  }
  return out;
}

/**
 * A click selection (B1): a plain click selects only `id`, Ctrl (or ⌘) toggles it, Shift selects the
 * range from the anchor in `order` (the console's order). Returns { ids, anchor }.
 */
export function pickIds(ids, anchor, id, { ctrl = false, shift = false } = {}, order = []) {
  if (shift && anchor && order.includes(anchor) && order.includes(id)) {
    const [a, b] = [order.indexOf(anchor), order.indexOf(id)].sort((x, y) => x - y);
    const range = order.slice(a, b + 1);
    const base = ctrl ? ids.filter((x) => !range.includes(x)) : [];
    return { ids: [...base, ...range], anchor };
  }
  if (ctrl) return ids.includes(id) ? { ids: ids.filter((x) => x !== id), anchor: id } : { ids: [...ids, id], anchor: id };
  return { ids: [id], anchor: id };
}
