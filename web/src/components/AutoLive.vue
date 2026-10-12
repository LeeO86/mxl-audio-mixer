<script setup>
// A live readout of the control engine (§8) for the Auto panel: a signal's voice dot, speech probability
// or auto gain, or whether a duck's keys speak. Its own component, so the meters (25 Hz) re-render only
// it and not the panel's inputs while someone types in them.
import { computed } from "vue";
import { fmtSigned } from "../api.js";
import { autoGain, live } from "../store.js";

const props = defineProps({
  kind: { type: String, required: true }, // "dot" | "probability" | "gain" | "duck"
  id: { type: String, default: "" },
  duck: { type: Object, default: null },
});
const vad = computed(() => live.vad[props.id] || null);
const gain = computed(() => {
  const g = autoGain(props.id);
  return g === undefined ? "" : Number.isFinite(g) ? fmtSigned(g) : "−∞";
});
const speaking = computed(() => !!props.duck?.enabled && props.duck.keys.some((k) => live.vad[k]?.active));
</script>

<template>
  <span v-if="kind === 'dot'" class="vad" :class="{ on: vad?.active }">●</span>
  <span v-else-if="kind === 'probability'" class="prob" :title="`probability ${(vad?.probability ?? 0).toFixed(2)}`"
    ><span :class="{ on: vad?.active }" :style="{ width: `${Math.round((vad?.probability ?? 0) * 100)}%` }"></span
  ></span>
  <span v-else-if="kind === 'gain'" class="num" :data-gain="id">{{ gain }}</span>
  <span v-else-if="kind === 'duck'" class="duckstate" :class="{ on: speaking }" :data-duck-active="speaking">{{ speaking ? "speech" : "—" }}</span>
</template>
