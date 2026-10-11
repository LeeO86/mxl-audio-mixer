<script setup>
// A compact control for a console strip (trim, pan, width, send level): a label, the value, and a slider.
// Double-click resets; while dragged it shows its own value.
import { computed, ref } from "vue";

const props = defineProps({
  label: { type: String, required: true },
  value: { type: Number, default: 0 },
  min: { type: Number, required: true },
  max: { type: Number, required: true },
  step: { type: Number, default: 0.5 },
  def: { type: Number, default: 0 },
  format: { type: Function, default: (v) => v.toFixed(1) },
  title: { type: String, default: "" },
});
const emit = defineEmits(["change"]);
const drag = ref(null);
const shown = computed(() => drag.value ?? props.value);

function input(ev) {
  drag.value = Number(ev.target.value);
  emit("change", drag.value);
}
function end(ev) {
  emit("change", Number(ev.target.value));
  drag.value = null;
}
</script>

<template>
  <label class="minipot" :title="title || `${label}: double-click resets`" @dblclick.prevent="emit('change', def)">
    <span class="mp-head"><span>{{ label }}</span><span class="num">{{ format(shown) }}</span></span>
    <input type="range" :min="min" :max="max" :step="step" :value="shown" :aria-label="label" @input="input" @change="end" />
  </label>
</template>
