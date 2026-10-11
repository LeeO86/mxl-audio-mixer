<script setup>
// A destructive action (§12.0): the first press arms it ("Press again"), a second press within 3 s runs it.
import { onUnmounted, ref } from "vue";

const props = defineProps({
  label: { type: String, required: true },
  small: { type: Boolean, default: false },
  disabled: { type: Boolean, default: false },
});
const emit = defineEmits(["confirm"]);
const armed = ref(false);
let timer = 0;

function press() {
  if (armed.value) {
    clearTimeout(timer);
    armed.value = false;
    emit("confirm");
    return;
  }
  armed.value = true;
  timer = setTimeout(() => (armed.value = false), 3000);
}
onUnmounted(() => clearTimeout(timer));
</script>

<template>
  <button class="btn" :class="[armed ? 'danger' : 'secondary', { small: props.small }]" :disabled="disabled" @click="press">
    {{ armed ? "Press again" : label }}
  </button>
</template>
