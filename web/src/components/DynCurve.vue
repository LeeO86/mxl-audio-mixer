<script setup>
// A dynamics node's static curve (§12.2): input level → output level from −80 to 0 dBFS, the threshold,
// and a dot at the live input peak and the output it gets after the measured gain reduction.
import { computed } from "vue";
import { transfer } from "../dsp.js";
import { dbValue } from "../api.js";

const props = defineProps({
  node: { type: Object, required: true },
  meter: { type: Object, default: null }, // { gain_reduction_db, in_peak_dbfs, out_peak_dbfs }
});
const S = 240;
const LO = -80;
const p = (db) => ((db - LO) / -LO) * S;
const path = computed(() => {
  const pts = [];
  for (let i = 0; i <= 160; i++) {
    const xin = LO + (i / 160) * -LO;
    const out = Math.max(LO, Math.min(0, transfer(props.node, xin)));
    pts.push(`${i ? "L" : "M"}${p(xin).toFixed(1)},${(S - p(out)).toFixed(1)}`);
  }
  return pts.join("");
});
const th = computed(() => (props.node.threshold_db ?? props.node.ceiling_db ?? null));
const dot = computed(() => {
  const i = dbValue(props.meter?.in_peak_dbfs);
  if (!Number.isFinite(i) || i < LO) return null;
  const o = i - (props.meter?.gain_reduction_db || 0) + (props.node.makeup_db || 0);
  return { cx: p(i), cy: S - p(Math.max(LO, Math.min(0, o))) };
});
const GRID = [-60, -40, -20, 0];
</script>

<template>
  <svg class="curve" :viewBox="`0 0 ${S} ${S}`" style="max-width: 180px; height: auto; aspect-ratio: 1" role="img" :aria-label="`${node.type} transfer curve`">
    <g class="grid">
      <line v-for="d in GRID" :key="`x${d}`" :x1="p(d)" :x2="p(d)" y1="0" :y2="S" />
      <line v-for="d in GRID" :key="`y${d}`" x1="0" :x2="S" :y1="S - p(d)" :y2="S - p(d)" />
      <text v-for="d in GRID.slice(0, 3)" :key="`t${d}`" :x="p(d) + 2" :y="S - 3">{{ d }}</text>
      <line x1="0" :y1="S" :x2="S" y2="0" />
    </g>
    <line v-if="th !== null" class="th" :x1="p(th)" :x2="p(th)" y1="0" :y2="S" />
    <path :d="path" class="resp" />
    <circle v-if="dot" class="dot" :cx="dot.cx" :cy="dot.cy" r="5" />
  </svg>
</template>
