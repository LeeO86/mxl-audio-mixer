<script setup>
// One parameter of a logic block in the list (§11.3), edited by its catalog kind.
//
// The field shows its own copy of the value. A pushed mix (another client, or this one's own change
// coming back) updates that copy only while the field is neither focused nor edited, so what is being
// typed or picked is never replaced; when the field is left, it shows the stored value again. If the
// stored value changes elsewhere meanwhile, the field is outlined in amber and says so; Enter still
// applies what was typed, Escape takes the stored value.
//
// A choice made with the keyboard (arrow keys step through a <select> and fire "change" at every step)
// applies on Enter or when the field is left, and Escape puts the stored one back, so stepping past
// strips does not move an action to each of them; a mouse choice applies at once.
import { ref, watch } from "vue";

const props = defineProps({
  block: { type: Object, required: true },
  param: { type: Object, required: true },
  options: { type: Function, required: true }, // (param, block) => [{ id, name }] or null
});
const emit = defineEmits(["set"]);
const NUMBER_KINDS = ["number", "integer"];
const JSON_KINDS = ["points", "source"];

const raw = () => props.block[props.param.name];
const empty = () => raw() === undefined || raw() === null || raw() === "";
// The stored value as the field shows it.
const stored = () => (JSON_KINDS.includes(props.param.kind) ? JSON.stringify(raw()) : empty() ? "" : String(raw()));

const field = ref(null);
const shown = ref(stored());
const dirty = ref(false); // typed or picked, not applied yet
const focused = () => field.value && document.activeElement === field.value;
const elsewhere = ref(null); // the stored value, when it changed while this field was being edited
watch(stored, (v) => {
  if (!dirty.value && !focused()) shown.value = v;
  else if (dirty.value) elsewhere.value = v;
});
const note = () => (elsewhere.value === null ? null : `changed elsewhere to ${elsewhere.value === "" ? "nothing" : elsewhere.value}; Enter keeps yours, Escape takes it`);

function typed(e) {
  shown.value = e.target.value;
  dirty.value = true;
}
function apply() {
  if (!dirty.value) return;
  dirty.value = false;
  elsewhere.value = null;
  emit("set", shown.value);
}
// Left without applying (or after applying): the stored value again, unless it is still on its way.
function left() {
  apply();
  setTimeout(() => {
    if (!dirty.value && !focused()) shown.value = stored();
  }, 1500);
}

const keyed = ref(false);
function picked(e) {
  shown.value = e.target.value;
  dirty.value = true;
  if (!keyed.value) apply();
}
function settle() {
  keyed.value = false;
  apply();
}
function cancel() {
  keyed.value = false;
  dirty.value = false;
  elsewhere.value = null;
  shown.value = stored();
}
</script>

<template>
  <select
    v-if="props.options(props.param, props.block)"
    ref="field"
    :value="shown"
    :aria-label="`${props.block.id} ${props.param.name}`"
    :title="note() || props.param.name"
    :class="{ elsewhere: elsewhere !== null }"
    @pointerdown="keyed = false"
    @keydown.enter="settle"
    @keydown.esc="cancel"
    @keydown="!['Enter', 'Tab', 'Escape'].includes($event.key) && (keyed = true)"
    @blur="
      settle();
      left();
    "
    @change="picked"
  >
    <option v-if="empty()" value="" disabled>{{ props.param.name }}…</option>
    <option v-for="o in props.options(props.param, props.block)" :key="o.id" :value="o.id">{{ o.name }}</option>
  </select>
  <label v-else-if="props.param.kind === 'boolean'" class="check"><input type="checkbox" :checked="raw()" :aria-label="`${props.block.id} ${props.param.name}`" @change="emit('set', $event.target.checked)" /> {{ props.param.name }}</label>
  <input
    v-else-if="NUMBER_KINDS.includes(props.param.kind)"
    ref="field"
    type="number"
    :min="props.param.min"
    :max="props.param.max"
    :step="props.param.kind === 'integer' ? 1 : 'any'"
    :value="shown"
    :aria-label="`${props.block.id} ${props.param.name}`"
    :title="note() || props.param.name"
    :class="{ elsewhere: elsewhere !== null }"
    style="width: 6rem"
    @input="typed"
    @change="apply"
    @keydown.esc="cancel"
    @blur="left"
  />
  <input
    v-else
    ref="field"
    :value="shown"
    :aria-label="`${props.block.id} ${props.param.name}`"
    :title="note() || (JSON_KINDS.includes(props.param.kind) ? `${props.param.name} (JSON)` : props.param.name)"
    :class="{ elsewhere: elsewhere !== null }"
    :placeholder="props.param.name"
    style="width: 12rem"
    @input="typed"
    @change="apply"
    @keydown.esc="cancel"
    @blur="left"
  />
</template>
