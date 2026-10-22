<script setup>
// The EQ editor's response (§12.2): the summed curve, each band's own (dashed), and a handle per band.
// Drag a handle for frequency and gain (a notch, a high- or low-pass band: frequency only), the wheel or
// the Q buttons for Q; a double-click resets that band (frequency and type stay; the rest as the band's
// reset buttons).
import { computed, ref } from "vue";
import { bandDb, filterDb, freqs } from "../dsp.js";
import { fmtHz } from "../api.js";

const props = defineProps({
  node: { type: Object, required: true },
  active: { type: Number, default: -1 },
  range: { type: Number, default: 24 },
});
const emit = defineEmits(["bands", "select"]);

const W = 480;
const H = 180;
const PAD = 26;
const F = freqs(240);
const x = (f) => PAD + (Math.log10(f / 20) / 3) * (W - 2 * PAD);
const fOf = (px) => 20 * Math.pow(10, ((px - PAD) / (W - 2 * PAD)) * 3);
const y = (db) => H / 2 - (db / props.range) * (H / 2 - 12);
const dbOf = (py) => ((H / 2 - py) / (H / 2 - 12)) * props.range;

const bands = computed(() => props.node.bands || []);
const bandType = (b) => (props.node.type === "dynamic-eq" ? "bell" : b.type);
// Each band's response in dB at f (high- and low-pass bands: Butterworth of their slope, D4).
const curves = computed(() => bands.value.map((b) => (f) => bandDb({ ...b, type: bandType(b) }, f)));
const isPass = (b) => bandType(b) === "high-pass" || bandType(b) === "low-pass";
const path = (fn) => F.map((f, i) => `${i ? "L" : "M"}${x(f).toFixed(1)},${y(Math.max(-props.range, Math.min(props.range, fn(f)))).toFixed(1)}`).join("");
const total = computed(() => {
  if (props.node.type === "filter") return path((f) => filterDb(props.node, f));
  return path((f) => curves.value.reduce((s, c) => s + c(f), 0));
});
const each = computed(() => (bands.value.length > 1 ? curves.value.map((c) => path(c)) : []));
const handles = computed(() => {
  if (props.node.type === "filter") return [{ i: 0, cx: x(props.node.freq_hz), cy: y(-3) }];
  return bands.value.map((b, i) => ({ i, cx: x(b.freq_hz), cy: y(isPass(b) ? -3 : bandType(b) === "notch" ? 0 : b.gain_db) }));
});
const GRID_F = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
const gridDb = computed(() => [-props.range, -props.range / 2, 0, props.range / 2, props.range]);

const svg = ref(null);
let dragging = -1;
function toSvg(ev) {
  const r = svg.value.getBoundingClientRect();
  return [((ev.clientX - r.left) / r.width) * W, ((ev.clientY - r.top) / r.height) * H];
}
function round(v, d) {
  const p = Math.pow(10, d);
  return Math.round(v * p) / p;
}
function down(i, ev) {
  dragging = i;
  emit("select", i);
  ev.currentTarget.setPointerCapture?.(ev.pointerId);
  ev.preventDefault();
}
function move(ev) {
  if (dragging < 0) return;
  const [px, py] = toSvg(ev);
  const f = Math.min(20000, Math.max(10, fOf(px)));
  if (props.node.type === "filter") {
    emit("bands", { freq_hz: round(f, f < 100 ? 1 : 0) });
    return;
  }
  const next = bands.value.map((b) => ({ ...b }));
  const b = next[dragging];
  b.freq_hz = round(f, f < 100 ? 1 : 0);
  if (bandType(b) !== "notch" && !isPass(b)) b.gain_db = round(Math.min(24, Math.max(-24, dbOf(py))), 1);
  emit("bands", { bands: next });
}
function up() {
  dragging = -1;
}
// A band's values after a reset (the defaults of its pots in the node editor).
const RESET = { gain_db: 0, q: 1 };
const RESET_DYNAMIC = { ...RESET, threshold_db: -30, ratio: 3, attack_ms: 5, release_ms: 100 };
function reset(i) {
  if (props.node.type === "filter") return;
  dragging = -1;
  const next = bands.value.map((b) => ({ ...b }));
  Object.assign(next[i], props.node.type === "dynamic-eq" ? RESET_DYNAMIC : RESET);
  emit("bands", { bands: next });
}
function wheel(i, ev) {
  if (props.node.type === "filter" || isPass(bands.value[i])) return;
  ev.preventDefault();
  const next = bands.value.map((b) => ({ ...b }));
  next[i].q = round(Math.min(40, Math.max(0.1, next[i].q * (ev.deltaY < 0 ? 1.1 : 1 / 1.1))), 2);
  emit("bands", { bands: next });
}
</script>

<template>
  <svg ref="svg" class="curve" :viewBox="`0 0 ${W} ${H}`" role="img" :aria-label="`${node.type} response`" @pointermove="move" @pointerup="up" @pointercancel="up">
    <g class="grid">
      <line v-for="f in GRID_F" :key="f" :x1="x(f)" :x2="x(f)" y1="0" :y2="H" />
      <text v-for="f in GRID_F" :key="`t${f}`" :x="x(f) + 2" :y="H - 4">{{ fmtHz(f) }}</text>
      <line v-for="d in gridDb" :key="`d${d}`" :x1="PAD" :x2="W - PAD" :y1="y(d)" :y2="y(d)" />
      <text v-for="d in gridDb" :key="`dt${d}`" x="2" :y="y(d) + 3">{{ d > 0 ? `+${d}` : d }}</text>
    </g>
    <path v-for="(p, i) in each" :key="i" :d="p" class="band" />
    <path :d="total" class="resp" />
    <g v-for="h in handles" :key="h.i" class="handle" :class="{ active: h.i === active }" @pointerdown="down(h.i, $event)" @dblclick="reset(h.i)" @wheel="wheel(h.i, $event)">
      <circle :cx="h.cx" :cy="h.cy" r="10" :aria-label="`band ${h.i + 1}`"><title>drag: frequency and gain; wheel: Q; double-click: reset</title></circle>
      <text :x="h.cx" :y="h.cy + 4" text-anchor="middle" font-size="11" style="pointer-events: none">{{ h.i + 1 }}</text>
    </g>
  </svg>
</template>
