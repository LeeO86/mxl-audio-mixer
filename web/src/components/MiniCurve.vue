<script setup>
// The chain overview's response: the sum of the EQ-like nodes (EQ, filters, dynamic EQ at full depth),
// ±18 dB over 20 Hz – 20 kHz.
import { computed } from "vue";
import { freqs, nodeDb } from "../dsp.js";

const props = defineProps({
  chain: { type: Array, default: () => [] },
  width: { type: Number, default: 92 },
  height: { type: Number, default: 30 },
});
const F = freqs(60);
const path = computed(() => {
  const nodes = props.chain.filter((n) => ["eq", "filter", "dynamic-eq"].includes(n.type));
  return F.map((f, i) => {
    const db = Math.max(-18, Math.min(18, nodes.reduce((s, n) => s + nodeDb(n, f), 0)));
    const x = (i / (F.length - 1)) * props.width;
    const y = props.height / 2 - (db / 18) * (props.height / 2 - 1);
    return `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`;
  }).join("");
});
const flat = computed(() => !props.chain.some((n) => ["eq", "filter", "dynamic-eq"].includes(n.type) && !n.bypass));
</script>

<template>
  <svg class="minicurve" :viewBox="`0 0 ${width} ${height}`" :width="width" :height="height" aria-hidden="true">
    <line x1="0" :y1="height / 2" :x2="width" :y2="height / 2" class="axis" />
    <path :d="path" :class="{ flat }" />
  </svg>
</template>
