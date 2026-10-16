<script setup>
// One parameter of a logic block in the list (§11.3), edited by its catalog kind. A component of its own:
// the Logic tab redraws often (ports, actions, selection), and a redraw of a plain input would put the
// stored value back over a number being typed. A choice made with the keyboard (arrow keys step through
// a <select> and fire "change" at every step) applies on Enter or when the field is left, and Escape
// puts the stored one back, so stepping past strips does not move an action to each of them; a mouse
// choice applies at once.
import { ref } from "vue";

const props = defineProps({
  block: { type: Object, required: true },
  param: { type: Object, required: true },
  options: { type: Function, required: true }, // (param, block) => [{ id, name }] or null
});
const emit = defineEmits(["set"]);
const NUMBER_KINDS = ["number", "integer"];
const JSON_KINDS = ["points", "source"];
const keyed = ref(false);
const pending = ref(null);
function picked(value) {
  if (keyed.value) pending.value = value;
  else emit("set", value);
}
function settle() {
  if (pending.value !== null) emit("set", pending.value);
  pending.value = null;
  keyed.value = false;
}
function cancel(e) {
  pending.value = null;
  keyed.value = false;
  e.target.value = empty() ? "" : props.block[props.param.name];
}
const empty = () => props.block[props.param.name] === undefined || props.block[props.param.name] === null || props.block[props.param.name] === "";
</script>

<template>
  <select
    v-if="props.options(props.param, props.block)"
    :value="empty() ? '' : props.block[props.param.name]"
    :aria-label="`${props.block.id} ${props.param.name}`"
    :title="props.param.name"
    @pointerdown="keyed = false"
    @keydown.enter="settle"
    @keydown.esc="cancel"
    @keydown="!['Enter', 'Tab', 'Escape'].includes($event.key) && (keyed = true)"
    @blur="settle"
    @change="picked($event.target.value)"
  >
    <option v-if="empty()" value="" disabled>{{ props.param.name }}…</option>
    <option v-for="o in props.options(props.param, props.block)" :key="o.id" :value="o.id">{{ o.name }}</option>
  </select>
  <label v-else-if="props.param.kind === 'boolean'" class="check"><input type="checkbox" :checked="props.block[props.param.name]" :aria-label="`${props.block.id} ${props.param.name}`" @change="emit('set', $event.target.checked)" /> {{ props.param.name }}</label>
  <input
    v-else-if="NUMBER_KINDS.includes(props.param.kind)"
    type="number"
    :min="props.param.min"
    :max="props.param.max"
    :step="props.param.kind === 'integer' ? 1 : 'any'"
    :value="props.block[props.param.name]"
    :aria-label="`${props.block.id} ${props.param.name}`"
    :title="props.param.name"
    style="width: 6rem"
    @change="emit('set', $event.target.value)"
  />
  <input v-else-if="JSON_KINDS.includes(props.param.kind)" :value="JSON.stringify(props.block[props.param.name])" :aria-label="`${props.block.id} ${props.param.name}`" :title="`${props.param.name} (JSON)`" style="width: 12rem" @change="emit('set', $event.target.value)" />
  <input v-else :value="props.block[props.param.name]" :aria-label="`${props.block.id} ${props.param.name}`" :placeholder="props.param.name" style="width: 12rem" @change="emit('set', $event.target.value)" />
</template>
