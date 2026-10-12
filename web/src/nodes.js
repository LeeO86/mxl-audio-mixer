// Processing node types (README "Processing"): label, a valid new node, and the parameters the editor
// shows. Ranges follow the server's validation (src/mix/chain.rs).

const THRESHOLD = { min: -100, max: 0, def: -20, fine: 0.5, unit: "dB" };
const ATTACK = { min: 0.1, max: 1000, def: 10, fine: 0.1, digits: 1, unit: "ms", log: true };
const RELEASE = { min: 1, max: 10000, def: 150, fine: 0.1, digits: 0, unit: "ms", log: true };
const RATIO = { min: 1, max: 100, def: 3, fine: 0.05, digits: 1, unit: ": 1", log: true };
const FREQ = { min: 10, max: 20000, def: 1000, fine: 0.02, digits: 0, unit: "Hz", log: true };

export const NODE_TYPES = {
  eq: {
    label: "EQ",
    group: "EQ",
    make: () => ({ type: "eq", bands: [{ type: "bell", freq_hz: 1000, q: 1, gain_db: 0 }] }),
    fields: [],
  },
  filter: {
    label: "Filter",
    group: "EQ",
    make: () => ({ type: "filter", mode: "high-pass", freq_hz: 80, slope_db_per_oct: 24 }),
    fields: [
      { key: "mode", label: "Mode", options: [["high-pass", "High-pass"], ["low-pass", "Low-pass"]] },
      { key: "slope_db_per_oct", label: "Slope dB/oct", options: [[12, "12"], [24, "24"], [36, "36"], [48, "48"]] },
      { key: "freq_hz", label: "Frequency", ...FREQ, def: 80 },
    ],
  },
  "dynamic-eq": {
    label: "Dynamic EQ",
    group: "EQ",
    make: () => ({
      type: "dynamic-eq",
      bands: [{ freq_hz: 3000, q: 2, gain_db: -6, threshold_db: -30, ratio: 3, attack_ms: 5, release_ms: 100 }],
    }),
    fields: [],
  },
  compressor: {
    label: "Compressor",
    group: "Dynamics",
    make: () => ({ type: "compressor", threshold_db: -20, ratio: 3, knee_db: 6, attack_ms: 10, release_ms: 150, makeup_db: 0, detector: "rms" }),
    fields: [
      { key: "threshold_db", label: "Threshold", ...THRESHOLD },
      { key: "ratio", label: "Ratio", ...RATIO, max: 1000 },
      { key: "knee_db", label: "Knee", min: 0, max: 24, def: 6, fine: 0.5, unit: "dB" },
      { key: "attack_ms", label: "Attack", ...ATTACK },
      { key: "release_ms", label: "Release", ...RELEASE },
      { key: "makeup_db", label: "Makeup", min: -24, max: 24, def: 0, fine: 0.5, unit: "dB" },
      { key: "detector", label: "Detector", options: [["peak", "Peak"], ["rms", "RMS"]] },
      { key: "sidechain_hpf_hz", label: "Sidechain HPF", ...FREQ, def: 100, optional: true },
      { key: "key", label: "Key", key_source: true, optional: true },
    ],
  },
  limiter: {
    label: "Limiter",
    group: "Dynamics",
    make: () => ({ type: "limiter", ceiling_db: -1, release_ms: 50, lookahead_ms: 1.5, true_peak: true }),
    fields: [
      { key: "ceiling_db", label: "Ceiling", min: -40, max: 0, def: -1, fine: 0.1, unit: "dB" },
      { key: "release_ms", label: "Release", ...RELEASE, max: 5000, def: 50 },
      { key: "lookahead_ms", label: "Look-ahead", min: 0.1, max: 20, def: 1.5, fine: 0.1, unit: "ms" },
      { key: "true_peak", label: "True peak", bool: true },
    ],
  },
  expander: {
    label: "Expander",
    group: "Dynamics",
    make: () => ({ type: "expander", threshold_db: -45, ratio: 2, range_db: 20, attack_ms: 2, release_ms: 100 }),
    fields: [
      { key: "threshold_db", label: "Threshold", ...THRESHOLD, def: -45 },
      { key: "ratio", label: "Ratio", ...RATIO, def: 2 },
      { key: "range_db", label: "Range", min: 0, max: 100, def: 20, fine: 0.5, unit: "dB" },
      { key: "attack_ms", label: "Attack", ...ATTACK, def: 2 },
      { key: "release_ms", label: "Release", ...RELEASE, def: 100 },
    ],
  },
  gate: {
    label: "Gate",
    group: "Dynamics",
    make: () => ({ type: "gate", threshold_db: -50, hysteresis_db: 4, attack_ms: 0.5, hold_ms: 50, release_ms: 150, range_db: 40 }),
    fields: [
      { key: "threshold_db", label: "Threshold", ...THRESHOLD, def: -50 },
      { key: "hysteresis_db", label: "Hysteresis", min: 0, max: 24, def: 4, fine: 0.5, unit: "dB" },
      { key: "attack_ms", label: "Attack", ...ATTACK, def: 0.5 },
      { key: "hold_ms", label: "Hold", min: 0, max: 5000, def: 50, fine: 1, digits: 0, unit: "ms" },
      { key: "release_ms", label: "Release", ...RELEASE },
      { key: "range_db", label: "Range", min: 0, max: 100, def: 40, fine: 0.5, unit: "dB" },
    ],
  },
  "de-esser": {
    label: "De-esser",
    group: "Dynamics",
    make: () => ({ type: "de-esser", mode: "split-band", freq_hz: 6000, threshold_db: -30, range_db: 6, attack_ms: 1, release_ms: 80, listen: false }),
    fields: [
      { key: "mode", label: "Mode", options: [["split-band", "Split-band"], ["wide-band", "Wide-band"]] },
      { key: "freq_hz", label: "Frequency", ...FREQ, min: 2000, max: 12000, def: 6000 },
      { key: "threshold_db", label: "Threshold", ...THRESHOLD, def: -30 },
      { key: "range_db", label: "Range", min: 0, max: 40, def: 6, fine: 0.5, unit: "dB" },
      { key: "attack_ms", label: "Attack", ...ATTACK, def: 1 },
      { key: "release_ms", label: "Release", ...RELEASE, def: 80 },
      { key: "listen", label: "Listen (the band the de-esser hears)", bool: true },
    ],
  },
  ducker: {
    label: "Ducker",
    group: "Dynamics",
    make: (others) => ({
      type: "ducker",
      key: { signal: others[0] || "", tap: "post-fader" },
      threshold_db: -40,
      depth_db: 12,
      attack_ms: 5,
      hold_ms: 200,
      release_ms: 500,
    }),
    fields: [
      { key: "key", label: "Key", key_source: true },
      { key: "threshold_db", label: "Threshold", ...THRESHOLD, def: -40 },
      { key: "depth_db", label: "Depth", min: 0, max: 100, def: 12, fine: 0.5, unit: "dB" },
      { key: "attack_ms", label: "Attack", ...ATTACK, def: 5 },
      { key: "hold_ms", label: "Hold", min: 0, max: 5000, def: 200, fine: 1, digits: 0, unit: "ms" },
      { key: "release_ms", label: "Release", ...RELEASE, def: 500 },
    ],
  },
  multiband: {
    label: "Multiband",
    group: "Dynamics",
    make: () => ({
      type: "multiband",
      crossovers_hz: [200, 2000],
      bands: [0, 1, 2].map(() => ({ threshold_db: -20, ratio: 2, attack_ms: 10, release_ms: 150, makeup_db: 0 })),
    }),
    fields: [],
  },
  oscillator: {
    label: "Oscillator",
    group: "Other",
    make: () => ({ type: "oscillator", waveform: "sine", freq_hz: 1000, level_dbfs: -18 }),
    fields: [
      { key: "waveform", label: "Waveform", options: [["sine", "Sine"], ["ebu-ident", "EBU stereo ident"], ["white", "White noise"], ["pink", "Pink noise"], ["sweep", "Sweep"], ["second-pulse", "Second pulse (latency)"]] },
      { key: "freq_hz", label: "Frequency", min: 20, max: 20000, def: 1000, fine: 0.02, digits: 0, unit: "Hz", log: true, when: (n) => n.waveform !== "white" && n.waveform !== "pink" },
      { key: "sweep_to_hz", label: "Sweep to", min: 20, max: 20000, def: 20000, fine: 0.02, digits: 0, unit: "Hz", log: true, when: (n) => n.waveform === "sweep" },
      { key: "sweep_s", label: "Sweep time", min: 0.1, max: 600, def: 10, fine: 0.1, digits: 1, unit: "s", when: (n) => n.waveform === "sweep" },
      { key: "level_dbfs", label: "Level", min: -60, max: 0, def: -18, fine: 0.5, unit: "dBFS (noise: RMS)" },
    ],
  },
  delay: {
    label: "Delay",
    group: "Other",
    make: () => ({ type: "delay", time: 10, unit: "ms" }),
    fields: [
      { key: "unit", label: "Unit", options: [["ms", "ms"], ["samples", "Samples"], ["frames", "Frames"]] },
      { key: "time", label: "Time", min: 0, max: 96000, def: 0, fine: 1, digits: 2 },
      { key: "frame_rate", label: "Frame rate", min: 1, max: 120, def: 25, fine: 0.01, digits: 3, unit: "fps", when: (n) => n.unit === "frames" },
    ],
  },
};

export const BAND_TYPES = [
  ["bell", "Bell"],
  ["low-shelf", "Low shelf"],
  ["high-shelf", "High shelf"],
  ["notch", "Notch"],
];

export const TAPS = [
  ["input", "Input"],
  ["pre-eq", "Pre-EQ"],
  ["post-eq", "Post-EQ"],
  ["pre-dynamics", "Pre-dynamics"],
  ["post-dynamics", "Post-dynamics"],
  ["pre-fader", "Pre-fader"],
  ["post-fader", "Post-fader"],
  ["post-pan", "Post-pan"],
];

export const tapLabel = (tap) => (typeof tap === "object" && tap ? `after ${tap.after}` : TAPS.find((t) => t[0] === tap)?.[1] || tap);

/** Nodes whose gain reduction is metered. */
export const DYNAMICS = new Set(["compressor", "limiter", "expander", "gate", "de-esser", "ducker", "multiband", "dynamic-eq"]);
export const EQ_LIKE = new Set(["eq", "filter", "dynamic-eq"]);
