<script setup>
// The surround panner's position (D5): the speakers of its layout on a circle (front at the top), the
// source where it sits; click or drag on the pad to move it. Spread shows as a ring around the source,
// divergence as the share drawn towards L and R.
import { computed, ref } from "vue";

const props = defineProps({ node: { type: Object, required: true } });
const emit = defineEmits(["change"]);

const S = 160;
const C = S / 2;
const R = 58;
const SPEAKERS = {
  "5.1": [["L", -30], ["R", 30], ["C", 0], ["Ls", -110], ["Rs", 110]],
  "7.1": [["L", -30], ["R", 30], ["C", 0], ["Lss", -90], ["Rss", 90], ["Lrs", -150], ["Rrs", 150]],
};
const at = (deg, r = R) => [C + r * Math.sin((deg * Math.PI) / 180), C - r * Math.cos((deg * Math.PI) / 180)];
const speakers = computed(() => (SPEAKERS[props.node.layout] || SPEAKERS["5.1"]).map(([label, a]) => ({ label, a, xy: at(a) })));
const source = computed(() => at(props.node.azimuth_deg || 0));
const spreadR = computed(() => 7 + ((props.node.spread_pct || 0) / 100) * 40);

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
  <svg ref="svg" class="surroundpad" :viewBox="`0 0 ${S} ${S}`" :width="S" :height="S" role="img" :aria-label="`surround position ${node.azimuth_deg}°`" @pointerdown="down" @pointermove="move" @pointerup="up" @pointercancel="up">
    <circle :cx="C" :cy="C" :r="R" class="ring" />
    <line :x1="C" :y1="C - R - 6" :x2="C" :y2="C - R + 6" class="front" />
    <g v-for="s in speakers" :key="s.label" class="spk">
      <rect :x="s.xy[0] - 5" :y="s.xy[1] - 5" width="10" height="10" rx="2" />
      <text :x="at(s.a, R + 15)[0]" :y="at(s.a, R + 15)[1] + 3" text-anchor="middle">{{ s.label }}</text>
    </g>
    <circle :cx="source[0]" :cy="source[1]" :r="spreadR" class="spread" />
    <circle :cx="source[0]" :cy="source[1]" r="6" class="source" />
  </svg>
</template>

<style scoped>
.surroundpad {
  display: block;
  touch-action: none;
  cursor: crosshair;
  margin-bottom: 0.4rem;
}
.ring {
  fill: none;
  stroke: var(--border);
}
.front {
  stroke: var(--muted);
  stroke-width: 2;
}
.spk rect {
  fill: var(--panel2);
  stroke: var(--muted);
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
