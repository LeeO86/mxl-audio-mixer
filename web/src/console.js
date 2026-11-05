// Helpers of the console strips: channel labels, the linked input patch (C3), numbered names (C6).

const LABELS = {
  mono: ["M"],
  stereo: ["L", "R"],
  "3.0": ["L", "R", "C"],
  "5.1": ["L", "R", "C", "LFE", "Ls", "Rs"],
  "7.1": ["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs"],
  "5.1.4": ["L", "R", "C", "LFE", "Ls", "Rs", "Ltf", "Rtf", "Ltr", "Rtr"],
  "7.1.4": ["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs", "Ltf", "Rtf", "Ltr", "Rtr"],
};

/** The channel labels of a format (a custom one lists its own). */
export function channelLabels(format) {
  if (LABELS[format]) return LABELS[format];
  if (Array.isArray(format?.custom)) return format.custom.map(String);
  return [];
}

/** Whether a patch is "linked": unpatched, or channels in order from the first one on the same input
 * (or signal at the same tap, or virtual output: D1). */
export function isLinked(patch) {
  if (patch.every((p) => !p)) return true;
  const first = patch[0];
  if (!first || "talkback" in first) return false;
  const same = (p) => JSON.stringify({ ...p, channel: 0 }) === JSON.stringify({ ...first, channel: 0 });
  return patch.every((p, i) => p && same(p) && p.channel === first.channel + i);
}

/** The whole patch from its first channel: the others follow in order on the same source. */
export function linkedPatch(n, first) {
  if (!first) return Array(n).fill(null);
  return Array.from({ length: n }, (_, i) => ({ ...first, channel: first.channel + i }));
}

/** `count` names for new strips: the name as it is for one, numbered ("Guest 1", "Guest 2") for more. */
export function numbered(base, count) {
  if (count <= 1) return [base];
  return Array.from({ length: count }, (_, i) => `${base} ${i + 1}`);
}

/**
 * A fader level typed by the operator (A5): "0", "-20", "−3,5", "+4", "-inf" or "−∞" ("inf" and "+inf"
 * are none). Below −80 dB is
 * −∞, above `max` is `max`. Returns dB (a number, −Infinity for −∞), or null for text that is no level.
 */
export function parseFaderDb(text, max) {
  const t = String(text).trim().toLowerCase().replace(/\s*db$/, "").replace(/[−–]/g, "-").replace(",", ".");
  if (/^-(inf|∞|oo)$/.test(t)) return -Infinity; // only minus infinity: "inf", "+inf" are no level
  if (!/^[+-]?(\d+\.?\d*|\.\d+)$/.test(t)) return null;
  const v = Number(t);
  if (v < -80) return -Infinity;
  return Math.min(max, Math.round(v * 10) / 10 || 0);
}

// ---- unfolded strips (A13) ----------------------------------------------------------------
const MAX_OFFSET = 100; // as the server's MAX_MEMBER_OFFSET_DB
const TRIM_RANGE = 42;
const FADER_MAX = 15;
const num = (v) => (v === "-inf" || v === null || v === undefined ? -Infinity : Number(v));
const out = (d) => (Number.isFinite(d) ? Math.round(d * 10) / 10 : "-inf");
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

/** The strip's members (one per channel), with the defaults for a strip that has none. */
export function membersOf(s, n) {
  return Array.from({ length: n }, (_, ch) => ({ fader_db: 0, trim_db: 0, mute: false, ...(s.members?.[ch] || {}) }));
}

/** What member `ch` shows: its fader (the strip's plus its offset, at most +15 dB), trim and mute. */
export function memberValue(s, n, ch) {
  const m = membersOf(s, n)[ch];
  const f = num(s.fader_db) + num(m.fader_db);
  return { fader_db: Number.isFinite(f) ? Math.min(FADER_MAX, f) : -Infinity, trim_db: clamp(s.trim_db + m.trim_db, -TRIM_RANGE, TRIM_RANGE), mute: !!s.mute || !!m.mute };
}

/**
 * The PATCH body for moving member `ch`'s `field` ("fader_db", "trim_db" or "mute") to `value`. Linked:
 * the strip's own value moves so that member lands there, every offset stays (a member at −∞ cannot
 * lead the fader); where the strip's range ends, the rest goes into that member's offset, so it still
 * lands there. A member's own mute is cleared by its button, linked or not. Unlinked: only that member's
 * offset changes; a strip fader at −∞ goes to 0 dB and the other members keep −∞.
 */
export function memberChange(s, n, ch, field, value) {
  const list = membersOf(s, n);
  const m = list[ch];
  if (field === "mute") {
    if (!value && m.mute) {
      list[ch] = { ...m, mute: false };
      return s.unlinked || !s.mute ? { members: list } : { members: list, mute: false };
    }
    if (!s.unlinked) return { mute: !!value };
    list[ch] = { ...m, mute: !!value };
    return { members: list };
  }
  if (field === "trim_db") {
    if (!s.unlinked) {
      const want = Math.round((value - m.trim_db) * 10) / 10;
      const trim = clamp(want, -TRIM_RANGE, TRIM_RANGE);
      if (trim === want) return { trim_db: trim };
      list[ch] = { ...m, trim_db: clamp(Math.round((value - trim) * 10) / 10, -MAX_OFFSET, MAX_OFFSET) };
      return { trim_db: trim, members: list };
    }
    list[ch] = { ...m, trim_db: clamp(Math.round((value - s.trim_db) * 10) / 10, -MAX_OFFSET, MAX_OFFSET) };
    return { members: list };
  }
  const d = num(value);
  const off = num(m.fader_db);
  if (!s.unlinked) {
    if (!Number.isFinite(off)) return null;
    if (!Number.isFinite(d)) return { fader_db: "-inf" };
    const want = d - off;
    if (want <= FADER_MAX) return { fader_db: out(want) };
    list[ch] = { ...m, fader_db: out(clamp(d - FADER_MAX, -MAX_OFFSET, MAX_OFFSET)) };
    return { fader_db: FADER_MAX, members: list };
  }
  const body = {};
  let base = num(s.fader_db);
  if (!Number.isFinite(base)) {
    base = 0;
    body.fader_db = 0;
    list.forEach((x, i) => i !== ch && (list[i] = { ...x, fader_db: "-inf" }));
  }
  list[ch] = { ...m, fader_db: Number.isFinite(d) ? out(clamp(d - base, -MAX_OFFSET, MAX_OFFSET)) : "-inf" };
  body.members = list;
  return body;
}

/** Whether any member has an offset or a mute of its own. */
export const hasOffsets = (s) => (s.members || []).some((m) => num(m.fader_db) !== 0 || m.trim_db || m.mute);
