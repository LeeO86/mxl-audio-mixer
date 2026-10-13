<script setup>
// A number input that keeps what is being typed: a value pushed meanwhile (another client, the mix coming
// back) shows once the field loses focus. Emits `change` with the number on Enter or blur.
import { ref, watch } from "vue";

const props = defineProps({
  value: { type: Number, required: true },
  min: { type: Number, default: undefined },
  max: { type: Number, default: undefined },
  step: { type: Number, default: 1 },
});
const emit = defineEmits(["change"]);
const text = ref(String(props.value));
const focused = ref(false);
watch(
  () => props.value,
  (v) => {
    if (!focused.value) text.value = String(v);
  },
);
function commit() {
  // v-model gives a number for type="number", or the text while it is no number yet.
  const raw = String(text.value).trim();
  const v = Number(raw);
  if (raw !== "" && Number.isFinite(v) && v !== props.value) emit("change", v);
}
function blur() {
  focused.value = false;
  commit();
  text.value = String(props.value);
}
</script>

<template>
  <input v-model="text" type="number" :min="min" :max="max" :step="step" @focus="focused = true" @blur="blur" @keydown.enter="commit" />
</template>
