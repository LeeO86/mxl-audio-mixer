<script setup>
// A small dialog over the page: a title, the content, Escape or a click beside it closes it.
import { onMounted, onUnmounted, ref } from "vue";

const props = defineProps({
  title: { type: String, required: true },
  wide: { type: Boolean, default: false },
});
const emit = defineEmits(["close"]);
const box = ref(null);
const key = (e) => e.key === "Escape" && emit("close");
onMounted(() => {
  window.addEventListener("keydown", key);
  box.value?.querySelector("input, select, button:not(.mclose)")?.focus();
});
onUnmounted(() => window.removeEventListener("keydown", key));
</script>

<template>
  <Teleport to="body">
    <div class="modal-back" @mousedown.self="emit('close')">
      <div ref="box" class="modal panel" :class="{ wide: props.wide }" role="dialog" :aria-label="props.title">
        <h3>{{ props.title }}<span class="spacer"></span><button class="btn small secondary mclose" aria-label="Close" @click="emit('close')">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
        </button></h3>
        <slot />
      </div>
    </div>
  </Teleport>
</template>
