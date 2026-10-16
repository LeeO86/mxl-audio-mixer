<script setup>
// One block on the logic canvas. Inputs are handles on the left, the output on the right; the live
// value (§11.2 debug view) is the pill at the bottom right, green while a boolean is on.
import { Handle, Position } from "@vue-flow/core";

const props = defineProps({ data: { type: Object, required: true } });
const top = (i, n) => `${((i + 1) / (n + 1)) * 100}%`;
</script>

<template>
  <div class="gnode lnode" :class="`k-${props.data.kind}`" :data-lnode="props.data.id">
    <Handle v-for="(p, i) in props.data.inputs" :id="p" :key="p" type="target" :position="Position.Left" :style="{ top: top(i, props.data.inputs.length) }" :title="p" />
    <div class="gname">{{ props.data.label }}</div>
    <div class="lfoot">
      <span class="gmeta">{{ props.data.note }}</span>
      <span v-if="props.data.live" class="lval" :class="{ on: props.data.on === true, off: props.data.on === false }" :data-live="props.data.id">{{ props.data.live }}</span>
    </div>
    <Handle v-if="props.data.output" id="out" type="source" :position="Position.Right" />
  </div>
</template>
