<script setup>
// Pick one of the 16 fixed colours (C2) or none. A colour from an older show that is not one of them
// shows as an extra swatch while it is set.
import { computed } from "vue";
import { COLOURS } from "../colours.js";

const props = defineProps({ value: { type: String, default: "" } });
const emit = defineEmits(["change"]);
const current = computed(() => (props.value || "").toLowerCase());
const custom = computed(() => (current.value && !COLOURS.some(([c]) => c === current.value) ? current.value : ""));
</script>

<template>
  <div class="swatches" role="radiogroup" aria-label="Colour">
    <button v-for="[c, name] in COLOURS" :key="c" class="swatch" role="radio" :aria-checked="current === c" :aria-label="name" :title="name" :style="{ background: c }" @click="emit('change', c)"></button>
    <button v-if="custom" class="swatch" role="radio" aria-checked="true" aria-label="Own colour" title="Own colour (from an older show)" :style="{ background: custom }"></button>
    <button class="swatch none" role="radio" :aria-checked="!current" aria-label="No colour" title="No colour" @click="emit('change', '')">✕</button>
  </div>
</template>

<style scoped>
.swatches {
  display: grid;
  grid-template-columns: repeat(9, 1.4rem);
  gap: 0.25rem;
}
.swatch {
  width: 1.4rem;
  height: 1.4rem;
  padding: 0;
  border-radius: 4px;
  border: 1px solid var(--border);
  cursor: pointer;
}
.swatch[aria-checked="true"] {
  outline: 2px solid var(--text, #fff);
  outline-offset: 1px;
}
.swatch.none {
  background: transparent;
  color: var(--muted);
  font-size: 0.7rem;
  line-height: 1;
}
</style>
