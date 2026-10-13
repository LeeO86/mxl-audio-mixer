<script setup>
// A live readout of the control engine (§8) for the Auto panel: a signal's voice dot, speech probability
// or auto gain, whether a duck's keys speak, a ratio duck's control or an auto-level's state. Its own
// component, so the meters (25 Hz) re-render only it and not the panel's inputs while someone types in them.
import { computed } from "vue";
import { fmtSigned } from "../api.js";
import { autoGain, live } from "../store.js";

const props = defineProps({
  kind: { type: String, required: true }, // "dot" | "probability" | "gain" | "duck" | "ratio" | "level"
  id: { type: String, default: "" },
  duck: { type: Object, default: null },
});
const vad = computed(() => live.vad[props.id] || null);
const gain = computed(() => {
  const g = autoGain(props.id);
  return g === undefined ? "" : Number.isFinite(g) ? fmtSigned(g) : "−∞";
});
const speaking = computed(() => !!props.duck?.enabled && props.duck.keys.some((k) => live.vad[k]?.active));
const STATES = { release: "released", duck: "ducking", interview: "interview", overvoice: "over-voice" };
const control = computed(() => live.duckControl[props.duck?.id] || null);
const level = computed(() => live.autolevels[props.id] || null);
const lufs = (v) => (v === null || v === undefined ? "—" : v.toFixed(1));
</script>

<template>
  <span v-if="kind === 'dot'" class="vad" :class="{ on: vad?.active }">●</span>
  <span v-else-if="kind === 'probability'" class="prob" :title="`probability ${(vad?.probability ?? 0).toFixed(2)}`"
    ><span :class="{ on: vad?.active }" :style="{ width: `${Math.round((vad?.probability ?? 0) * 100)}%` }"></span
  ></span>
  <span v-else-if="kind === 'gain'" class="num" :data-gain="id">{{ gain }}</span>
  <span v-else-if="kind === 'duck'" class="duckstate" :class="{ on: speaking }" :data-duck-active="speaking">{{ speaking ? "speech" : "—" }}</span>
  <span
    v-else-if="kind === 'ratio'"
    class="duckstate"
    :class="{ on: control && (control.state === 'duck' || control.state === 'overvoice') }"
    :data-duck-state="control?.state || ''"
    :title="control ? `stages: slow ${control.slow_db} dB, mid ${control.mid_db} dB, fast ${control.fast_db} dB; dialogue ${lufs(control.dialogue_lufs)} LUFS, bed ${lufs(control.bed_lufs)} LUFS` : ''"
    >{{ control ? `${STATES[control.state] || control.state} ${control.reduction_db >= 0.05 ? `−${control.reduction_db.toFixed(1)}` : "0.0"} dB · ${lufs(control.ratio_lu)}/${control.target_lu} LU` : "—" }}</span
  >
  <span
    v-else-if="kind === 'level'"
    class="num"
    :data-level="id"
    :title="level ? `level ${lufs(level.level_lufs)} LUFS, target ${level.target_lufs} LUFS${level.speech ? ' (speech)' : ''}` : ''"
    >{{ level ? `${fmtSigned(level.gain_db)} dB · ${lufs(level.level_lufs)}${level.frozen ? " frozen" : ""}` : "—" }}</span
  >
</template>
