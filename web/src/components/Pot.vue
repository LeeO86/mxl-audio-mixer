<script setup>
// One parameter as a "pot" (the family's fader): name, exact value, reset, and a slider between − and +
// buttons for fine steps. A held button repeats (mouse and touch). While the slider is dragged or the
// value typed, the pot shows its own value, so pushes from the server do not make it jump. `log` makes
// the slider logarithmic (frequencies, times).
import { computed, onUnmounted, ref } from "vue";

const props = defineProps({
  label: { type: String, required: true },
  value: { type: Number, default: 0 },
  min: { type: Number, required: true },
  max: { type: Number, required: true },
  def: { type: Number, default: 0 }, // the reset value
  fine: { type: Number, default: 0.1 }, // one − / + step (log: a factor of 1 + fine)
  digits: { type: Number, default: 1 },
  unit: { type: String, default: "" },
  log: { type: Boolean, default: false },
  disabled: { type: Boolean, default: false },
});
const emit = defineEmits(["change"]);

const drag = ref(null);
const typed = ref(null);
const shown = computed(() => drag.value ?? props.value);
const clamp = (v) => Math.min(props.max, Math.max(props.min, v));
const round = (v) => {
  const f = Math.pow(10, props.digits);
  return clamp(Math.round(v * f) / f);
};
const STEPS = 1000;
const toSlider = (v) =>
  props.log
    ? (Math.log(clamp(v) / props.min) / Math.log(props.max / props.min)) * STEPS
    : ((clamp(v) - props.min) / (props.max - props.min)) * STEPS;
const fromSlider = (s) => (props.log ? props.min * Math.pow(props.max / props.min, s / STEPS) : props.min + (s / STEPS) * (props.max - props.min));

function onSlide(event) {
  drag.value = round(fromSlider(Number(event.target.value)));
  emit("change", drag.value);
}
function onSlideEnd(event) {
  emit("change", round(fromSlider(Number(event.target.value))));
  drag.value = null;
}
function onType(event) {
  const v = Number(event.target.value);
  typed.value = null;
  if (event.target.value !== "" && Number.isFinite(v)) emit("change", round(v));
}

// Steps within 500 ms of the last continue from it, not from the server's value (which lags).
let target = 0;
let lastStep = 0;
let hold = 0;
function from() {
  return performance.now() - lastStep < 500 ? target : props.value;
}
function step(dir) {
  target = round(props.log ? target * Math.pow(1 + props.fine, dir) : target + dir * props.fine);
  lastStep = performance.now();
  emit("change", target);
}
function press(dir, event) {
  if (event.button !== 0 || props.disabled) return;
  event.currentTarget.setPointerCapture?.(event.pointerId);
  target = from();
  step(dir);
  hold = setTimeout(function again() {
    step(dir);
    hold = setTimeout(again, 60);
  }, 400);
}
function release() {
  clearTimeout(hold);
  hold = 0;
}
function key(dir, event) {
  if (event.detail !== 0) return; // a pointer press was handled by press()
  target = from();
  step(dir);
}
onUnmounted(release);
</script>

<template>
  <div class="fader pot">
    <span class="name">{{ label }}<span v-if="unit" class="muted small">&nbsp;{{ unit }}</span></span>
    <input
      class="value"
      type="number"
      :min="min"
      :max="max"
      :step="Math.pow(10, -digits)"
      :disabled="disabled"
      :value="typed ?? shown.toFixed(digits)"
      :aria-label="`${label} value`"
      @focus="typed = shown.toFixed(digits)"
      @input="typed = $event.target.value"
      @change="onType"
      @blur="typed = null"
    />
    <button class="btn small secondary" title="Reset" :aria-label="`Reset ${label}`" :disabled="disabled || value === def" @click="emit('change', def)">↺</button>
    <div class="track">
      <button
        class="btn secondary step"
        :aria-label="`${label} down`"
        :disabled="disabled"
        @pointerdown="press(-1, $event)"
        @pointerup="release"
        @pointercancel="release"
        @lostpointercapture="release"
        @click="key(-1, $event)"
        @contextmenu.prevent
      >
        −
      </button>
      <input type="range" min="0" :max="STEPS" step="1" :disabled="disabled" :value="toSlider(shown)" :aria-label="label" @input="onSlide" @change="onSlideEnd" />
      <button
        class="btn secondary step"
        :aria-label="`${label} up`"
        :disabled="disabled"
        @pointerdown="press(1, $event)"
        @pointerup="release"
        @pointercancel="release"
        @lostpointercapture="release"
        @click="key(1, $event)"
        @contextmenu.prevent
      >
        +
      </button>
    </div>
  </div>
</template>
