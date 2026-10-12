// Curves and scales drawn by the UI: EQ and filter responses (the RBJ biquads the DSP runs), dynamics
// transfer curves, the fader law and the meter scales.

const FS = 48000;

/** RBJ cookbook biquad of an EQ band (same formulas as src/dsp/biquad.rs). */
export function bandCoefficients(band) {
  const w = (2 * Math.PI * band.freq_hz) / FS;
  const cos = Math.cos(w);
  const sin = Math.sin(w);
  const alpha = sin / (2 * band.q);
  const a = Math.pow(10, (band.gain_db || 0) / 40);
  let b0, b1, b2, a0, a1, a2;
  switch (band.type) {
    case "low-shelf": {
      const s = 2 * Math.sqrt(a) * alpha;
      b0 = a * (a + 1 - (a - 1) * cos + s);
      b1 = 2 * a * (a - 1 - (a + 1) * cos);
      b2 = a * (a + 1 - (a - 1) * cos - s);
      a0 = a + 1 + (a - 1) * cos + s;
      a1 = -2 * (a - 1 + (a + 1) * cos);
      a2 = a + 1 + (a - 1) * cos - s;
      break;
    }
    case "high-shelf": {
      const s = 2 * Math.sqrt(a) * alpha;
      b0 = a * (a + 1 + (a - 1) * cos + s);
      b1 = -2 * a * (a - 1 + (a + 1) * cos);
      b2 = a * (a + 1 + (a - 1) * cos - s);
      a0 = a + 1 - (a - 1) * cos + s;
      a1 = 2 * (a - 1 - (a + 1) * cos);
      a2 = a + 1 - (a - 1) * cos - s;
      break;
    }
    case "notch":
      b0 = 1;
      b1 = -2 * cos;
      b2 = 1;
      a0 = 1 + alpha;
      a1 = -2 * cos;
      a2 = 1 - alpha;
      break;
    default: // bell
      b0 = 1 + alpha * a;
      b1 = -2 * cos;
      b2 = 1 - alpha * a;
      a0 = 1 + alpha / a;
      a1 = -2 * cos;
      a2 = 1 - alpha / a;
  }
  return [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0];
}

/** |H| in dB of a biquad at `f`. */
export function biquadDb([b0, b1, b2, a1, a2], f) {
  const w = (2 * Math.PI * f) / FS;
  const c1 = Math.cos(w);
  const s1 = Math.sin(w);
  const c2 = Math.cos(2 * w);
  const s2 = Math.sin(2 * w);
  const nr = b0 + b1 * c1 + b2 * c2;
  const ni = -(b1 * s1 + b2 * s2);
  const dr = 1 + a1 * c1 + a2 * c2;
  const di = -(a1 * s1 + a2 * s2);
  return 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di));
}

/** Butterworth high- or low-pass of `slope` dB/oct (order slope / 6) at `f`, in dB. */
export function filterDb(node, f) {
  const n = node.slope_db_per_oct / 6;
  const r = node.mode === "high-pass" ? node.freq_hz / f : f / node.freq_hz;
  return -10 * Math.log10(1 + Math.pow(r, 2 * n));
}

/** The magnitude of a node in dB at `f` (EQ-like nodes; others are flat). */
export function nodeDb(node, f) {
  if (node.bypass) return 0;
  if (node.type === "eq") return node.bands.reduce((s, b) => s + biquadDb(bandCoefficients(b), f), 0);
  if (node.type === "dynamic-eq") return node.bands.reduce((s, b) => s + biquadDb(bandCoefficients({ ...b, type: "bell" }), f), 0);
  if (node.type === "filter") return filterDb(node, f);
  return 0;
}

/** Log-spaced frequencies from 20 Hz to 20 kHz. */
export function freqs(n = 200) {
  return Array.from({ length: n }, (_, i) => 20 * Math.pow(1000, i / (n - 1)));
}

/** Static input → output level (dB) of a dynamics node. */
export function transfer(node, x) {
  const t = node.threshold_db;
  switch (node.type) {
    case "compressor": {
      const w = node.knee_db || 0;
      let y;
      if (2 * (x - t) < -w) y = x;
      else if (2 * Math.abs(x - t) <= w && w > 0) y = x + ((1 / node.ratio - 1) * Math.pow(x - t + w / 2, 2)) / (2 * w);
      else y = t + (x - t) / node.ratio;
      return y + (node.makeup_db || 0);
    }
    case "expander":
      return x >= t ? x : Math.max(t + (x - t) * node.ratio, x - node.range_db);
    case "gate":
      return x >= t ? x : x - node.range_db;
    case "limiter":
      return Math.min(x, node.ceiling_db);
    case "de-esser":
    case "ducker":
      return x;
    default:
      return x;
  }
}

// ---- fader law ---------------------------------------------------------------------
// Position 0…1 to dB: +15 (the most a fader goes) at the top, 0 dB at three quarters, −∞ at the bottom.
export const FADER_MAX_DB = 15;
const LAW = [
  [0, -Infinity],
  [0.03, -80],
  [0.12, -50],
  [0.25, -30],
  [0.4, -20],
  [0.55, -10],
  [0.65, -5],
  [0.75, 0],
  [0.835, 5],
  [0.92, 10],
  [1, 15],
];

export function posToDb(p) {
  if (p <= 0) return -Infinity;
  for (let i = 1; i < LAW.length; i++) {
    const [p1, d1] = LAW[i];
    const [p0, d0] = LAW[i - 1];
    if (p <= p1) return i === 1 ? -80 : d0 + ((p - p0) / (p1 - p0)) * (d1 - d0);
  }
  return FADER_MAX_DB;
}

export function dbToPos(db) {
  if (!Number.isFinite(db) || db <= -80) return db <= -80 && Number.isFinite(db) ? 0.03 : 0;
  for (let i = 2; i < LAW.length; i++) {
    const [p1, d1] = LAW[i];
    const [p0, d0] = LAW[i - 1];
    if (db <= d1) return p0 + ((db - d0) / (d1 - d0)) * (p1 - p0);
  }
  return 1;
}

export const FADER_MARKS = [15, 10, 5, 0, -5, -10, -20, -30, -50];

// ---- meter scales ------------------------------------------------------------------
// The digital scale (dBFS) and the PPM scales of IEC 60268-10 / EBU Tech 3205, placed relative to the
// alignment level (EBU R 68: −18 dBFS): EBU TEST and Nordic TEST at alignment, BBC 4 at alignment
// (4 dB a mark from 2 to 7, 6 dB from 1 to 2), DIN 0 dB at the permitted maximum (alignment + 9 dB).
export const SCALES = {
  dbfs: { label: "dBFS (sample peak)", ballistics: "peak", lo: -60, hi: 0, marks: [0, -6, -12, -18, -24, -30, -40, -50, -60].map((d) => [d, `${d}`]) },
  "true-peak": { label: "dBTP (true peak)", ballistics: "true-peak", lo: -60, hi: 0, marks: [0, -6, -12, -18, -24, -30, -40, -50, -60].map((d) => [d, `${d}`]) },
  ebu: { label: "PPM EBU (Type IIb)", ballistics: "ppm2", lo: -14, hi: 14, rel: true, marks: [12, 8, 4, 0, -4, -8, -12].map((d) => [d, d === 0 ? "TEST" : `${d > 0 ? "+" : ""}${d}`]) },
  bbc: { label: "PPM BBC (Type IIa)", ballistics: "ppm2", lo: -16, hi: 14, rel: true, marks: [[12, "7"], [8, "6"], [4, "5"], [0, "4"], [-4, "3"], [-8, "2"], [-14, "1"]] },
  nordic: { label: "PPM Nordic (Type I)", ballistics: "ppm1", lo: -36, hi: 12, rel: true, marks: [9, 6, 0, -6, -12, -18, -24, -30, -36].map((d) => [d, d === 0 ? "TEST" : `${d > 0 ? "+" : ""}${d}`]) },
  din: { label: "PPM DIN (Type I)", ballistics: "ppm1", lo: -50, hi: 5, rel: true, offset: 9, marks: [5, 0, -5, -10, -20, -30, -40, -50].map((d) => [d, `${d > 0 ? "+" : ""}${d}`]) },
};

/** The level of `values` (one signal's meters) for channel `c` on `scale`, in the scale's own units. */
export function scaleLevel(scale, values, c, alignment) {
  const s = SCALES[scale];
  const pick = { peak: values?.peak_dbfs, "true-peak": values?.true_peak_dbtp, ppm1: values?.ppm_type1_dbfs, ppm2: values?.ppm_type2_dbfs }[s.ballistics];
  const v = pick?.[c];
  const db = v === "-inf" || v === undefined ? -Infinity : v;
  return s.rel ? db - alignment - (s.offset || 0) : db;
}

/** Height fraction 0…1 of `level` on `scale`. */
export function scaleFraction(scale, level) {
  const s = SCALES[scale];
  if (!Number.isFinite(level)) return 0;
  return Math.min(1, Math.max(0, (level - s.lo) / (s.hi - s.lo)));
}
