<script setup>
// The surround panner's position (D5), seen from above: the speakers of its layout on a circle (front at
// the top), the source where it sits; click or drag on the pad to move it around (left/right and
// front/rear). The height layouts (5.1.4, 7.1.4) draw their top speakers on an inner ring; the source moves
// towards it as its height goes up (A10). Spread shows as a ring around the source, divergence as the
// share drawn towards L and R.
//
// Reusable (A8, the strip's miniature; a per-send panner can use it too): `node` is any object with
// `layout`, `azimuth_deg` and optionally `elevation_deg`, `spread_pct`; `size` in px; `mini` drops the
// labels. It emits `change` with `{ azimuth_deg }`.
import { computed, ref } from "vue";

const props = defineProps({
  node: { type: Object, required: true },
  size: { type: Number, default: 160 },
  mini: { type: Boolean, default: false },
});
const emit = defineEmits(["change"]);

const S = 160;
const C = S / 2;
const R = 58;
const RT = 34; // the top layer's ring
const BED = {
  "5.1": [["L", -30], ["R", 30], ["C", 0], ["Ls", -110], ["Rs", 110]],
  "7.1": [["L", -30], ["R", 30], ["C", 0], ["Lss", -90], ["Rss", 90], ["Lrs", -150], ["Rrs", 150]],
};
const TOP = [["Ltf", -45], ["Rtf", 45], ["Ltr", -135], ["Rtr", 135]];
const LAYOUTS = {
  "5.1": { bed: BED["5.1"], top: [] },
  "7.1": { bed: BED["7.1"], top: [] },
  "5.1.4": { bed: BED["5.1"], top: TOP },
  "7.1.4": { bed: BED["7.1"], top: TOP },
};
const at = (deg, r = R) => [C + r * Math.sin((deg * Math.PI) / 180), C - r * Math.cos((deg * Math.PI) / 180)];
const layout = computed(() => LAYOUTS[props.node.layout] || LAYOUTS["5.1"]);
const speakers = computed(() => layout.value.bed.map(([label, a]) => ({ label, a, xy: at(a) })));
const tops = computed(() => layout.value.top.map(([label, a]) => ({ label, a, xy: at(a, RT) })));
const height = computed(() => (tops.value.length ? Math.min(90, Math.max(0, props.node.elevation_deg || 0)) : 0));
const source = computed(() => at(props.node.azimuth_deg || 0, R - ((R - RT) * height.value) / 90));
const spreadR = computed(() => 7 + ((props.node.spread_pct || 0) / 100) * 40);
const label = computed(() => `surround position ${props.node.azimuth_deg || 0}°${tops.value.length ? `, height ${height.value}°` : ""}`);

const svg = ref(null);
let dragging = false;
function angleOf(ev) {
  const r = svg.value.getBoundingClientRect();
  const x = ((ev.clientX - r.left) / r.width) * S - C;
  const y = ((ev.clientY - r.top) / r.height) * S - C;
  return Math.round((Math.atan2(x, -y) * 180) / Math.PI);
}
function down(ev) {
  dragging = true;
  ev.currentTarget.setPointerCapture?.(ev.pointerId);
  emit("change", { azimuth_deg: angleOf(ev) });
}
function move(ev) {
  if (dragging) emit("change", { azimuth_deg: angleOf(ev) });
}
function up() {
  dragging = false;
}
</script>

<template>
  <svg ref="svg" class="surroundpad" :class="{ mini }" :viewBox="`0 0 ${S} ${S}`" :width="size" :height="size" role="img" :aria-label="label" @pointerdown="down" @pointermove="move" @pointerup="up" @pointercancel="up">
    <title v-if="mini">{{ label }}</title>
    <circle :cx="C" :cy="C" :r="R" class="ring" />
    <circle v-if="tops.length" :cx="C" :cy="C" :r="RT" class="ring top" />
    <line :x1="C" :y1="C - R - 6" :x2="C" :y2="C - R + 6" class="front" />
    <g v-for="s in speakers" :key="s.label" class="spk">
      <rect :x="s.xy[0] - 5" :y="s.xy[1] - 5" width="10" height="10" rx="2" />
      <text v-if="!mini" :x="at(s.a, R + 15)[0]" :y="at(s.a, R + 15)[1] + 3" text-anchor="middle">{{ s.label }}</text>
    </g>
    <g v-for="s in tops" :key="s.label" class="spk top">
      <rect :x="s.xy[0] - 4" :y="s.xy[1] - 4" width="8" height="8" rx="4" />
      <text v-if="!mini" :x="at(s.a, RT - 12)[0]" :y="at(s.a, RT - 12)[1] + 3" text-anchor="middle">{{ s.label }}</text>
    </g>
    <circle :cx="source[0]" :cy="source[1]" :r="spreadR" class="spread" />
    <circle :cx="source[0]" :cy="source[1]" :r="mini ? 9 : 6" class="source" />
  </svg>
</template>

<style scoped>
.surroundpad {
  display: block;
  touch-action: none;
  cursor: crosshair;
  margin-bottom: 0.4rem;
}
.surroundpad.mini {
  margin: 0 auto;
}
.ring {
  fill: none;
  stroke: var(--border);
}
.ring.top {
  stroke-dasharray: 3 3;
}
.front {
  stroke: var(--muted);
  stroke-width: 2;
}
.spk rect {
  fill: var(--panel2);
  stroke: var(--muted);
}
.spk.top rect {
  fill: var(--chip);
}
.spk text {
  font-size: 9px;
  fill: var(--muted);
  pointer-events: none;
}
.spread {
  fill: var(--accent);
  fill-opacity: 0.15;
  stroke: none;
}
.source {
  fill: var(--accent);
}
</style>
