<script setup>
// A signal's meter (§7): one bar per channel on the chosen scale (dBFS sample peak, true peak, or a PPM
// scale), the peak hold (click the readout to clear it), and the RMS as an inner bar on the dBFS scales.
import { computed } from "vue";
import { SCALES, scaleFraction, scaleLevel } from "../dsp.js";
import { api, dbValue, fmtDb } from "../api.js";
import { act } from "../store.js";

const props = defineProps({
  signal: { type: String, required: true },
  values: { type: Object, default: null }, // one signal of GET /api/v1/meters
  channels: { type: Number, default: 1 },
  scale: { type: String, default: "dbfs" },
  alignment: { type: Number, default: -18 },
  marks: { type: Boolean, default: true },
});

const s = computed(() => SCALES[props.scale] || SCALES.dbfs);
const chans = computed(() => Array.from({ length: props.channels }, (_, c) => c));
const level = (c) => scaleLevel(props.scale, props.values, c, props.alignment);
const height = (c) => `${scaleFraction(props.scale, level(c)) * 100}%`;
const digital = computed(() => !s.value.rel);
const rms = (c) => `${scaleFraction(props.scale, dbValue(props.values?.rms_dbfs?.[c])) * 100}%`;
const hold = (c) => dbValue(props.values?.peak_hold_dbfs?.[c]);
const holdPos = (c) => {
  const h = hold(c);
  const lvl = s.value.rel ? h - props.alignment - (s.value.offset || 0) : h;
  return `${scaleFraction(props.scale, lvl) * 100}%`;
};
const maxHold = computed(() => Math.max(...chans.value.map(hold)));
// Colour zones: the top 6 dB of the digital scales, above the permitted maximum (alignment + 9) on PPMs.
const zone = (c) => {
  const lvl = level(c);
  if (s.value.rel) {
    const over = lvl + (s.value.offset || 0) - 9;
    return over > 0 ? "bad" : over > -9 ? "warn" : "ok";
  }
  return lvl > -1 ? "bad" : lvl > -9 ? "warn" : "ok";
};
const markPos = (d) => `${scaleFraction(props.scale, d) * 100}%`;
function clear() {
  act(() => api.post(`/api/v1/meters/${encodeURIComponent(props.signal)}/clear`));
}
</script>

<template>
  <div class="meter" :title="`${s.label}; peak hold ${fmtDb(maxHold)} dBFS (click to clear)`">
    <div class="bars">
      <div v-for="c in chans" :key="c" class="bar">
        <span v-if="digital" class="rms" :style="{ height: rms(c) }"></span>
        <span class="lvl" :class="zone(c)" :style="{ height: height(c) }"></span>
        <span class="hold" :style="{ bottom: holdPos(c) }"></span>
      </div>
      <div v-if="marks" class="marks">
        <span v-for="[d, t] in s.marks" :key="t" :style="{ bottom: markPos(d) }">{{ t }}</span>
      </div>
    </div>
    <button class="holdval num" :class="{ over: maxHold > -1 }" :aria-label="`Clear the peak hold of ${signal}`" @click="clear">{{ fmtDb(maxHold) }}</button>
  </div>
</template>
