<script setup>
// The whole chain's response (§12.2): the sum of its active EQ, filter and dynamic EQ nodes (dynamic
// bands at full depth), ±24 dB over 20 Hz – 20 kHz.
import { computed } from "vue";
import { freqs, nodeDb } from "../dsp.js";
import { fmtHz } from "../api.js";

const props = defineProps({ chain: { type: Array, default: () => [] } });
const W = 800;
const H = 180;
const PAD = 26;
const R = 24;
const F = freqs(240);
const x = (f) => PAD + (Math.log10(f / 20) / 3) * (W - 2 * PAD);
const y = (db) => H / 2 - (db / R) * (H / 2 - 10);
const path = computed(() =>
  F.map((f, i) => {
    const db = Math.max(-R, Math.min(R, props.chain.reduce((s, n) => s + nodeDb(n, f), 0)));
    return `${i ? "L" : "M"}${x(f).toFixed(1)},${y(db).toFixed(1)}`;
  }).join(""),
);
const GRID_F = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
</script>

<template>
  <svg class="curve" :viewBox="`0 0 ${W} ${H}`" style="height: auto" role="img" aria-label="chain response">
    <g class="grid">
      <line v-for="f in GRID_F" :key="f" :x1="x(f)" :x2="x(f)" y1="0" :y2="H" />
      <text v-for="f in GRID_F" :key="`t${f}`" :x="x(f) + 2" :y="H - 4">{{ fmtHz(f) }}</text>
      <line v-for="d in [-24, -12, 0, 12, 24]" :key="`d${d}`" :x1="PAD" :x2="W - PAD" :y1="y(d)" :y2="y(d)" />
      <text v-for="d in [-24, -12, 0, 12, 24]" :key="`dt${d}`" x="2" :y="y(d) + 3">{{ d > 0 ? `+${d}` : d }}</text>
    </g>
    <path :d="path" class="resp" />
  </svg>
</template>
