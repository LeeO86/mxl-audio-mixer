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

/** Whether a patch is "linked": unpatched, or input channels in order from the first one. */
export function isLinked(patch) {
  if (patch.every((p) => !p)) return true;
  const first = patch[0];
  if (!first || !("input" in first)) return false;
  return patch.every((p, i) => p && "input" in p && p.input === first.input && p.channel === first.channel + i);
}

/** The whole patch from its first channel: the others follow in order on the same input. */
export function linkedPatch(n, first) {
  if (!first) return Array(n).fill(null);
  return Array.from({ length: n }, (_, i) => ({ input: first.input, channel: first.channel + i }));
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
