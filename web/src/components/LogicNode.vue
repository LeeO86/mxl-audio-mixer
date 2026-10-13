<script setup>
// One block on the logic canvas. Inputs are handles on the left, the output on the right.
import { Handle, Position } from "@vue-flow/core";

const props = defineProps({ data: { type: Object, required: true } });
const top = (i, n) => `${((i + 1) / (n + 1)) * 100}%`;
</script>

<template>
  <div class="gnode lnode" :data-lnode="props.data.id">
    <Handle v-for="(p, i) in props.data.inputs" :id="p" :key="p" type="target" :position="Position.Left" :style="{ top: top(i, props.data.inputs.length) }" :title="p" />
    <div class="gname">{{ props.data.label }}</div>
    <div class="gmeta">{{ props.data.note }}<template v-if="props.data.live"> · {{ props.data.live }}</template></div>
    <Handle v-if="props.data.output" id="out" type="source" :position="Position.Right" />
  </div>
</template>
