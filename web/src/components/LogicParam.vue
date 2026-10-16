<script setup>
// One parameter of a logic block in the list (§11.3), edited by its catalog kind. A component of its own:
// the Logic tab redraws often (ports, actions, selection), and a redraw of a plain input would put the
// stored value back over a number being typed.
const props = defineProps({
  block: { type: Object, required: true },
  param: { type: Object, required: true },
  options: { type: Function, required: true }, // (param, block) => [{ id, name }] or null
});
const emit = defineEmits(["set"]);
const NUMBER_KINDS = ["number", "integer"];
const JSON_KINDS = ["points", "source"];
</script>

<template>
  <select v-if="props.options(props.param, props.block)" :value="props.block[props.param.name]" :aria-label="`${props.block.id} ${props.param.name}`" :title="props.param.name" @change="emit('set', $event.target.value)">
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
