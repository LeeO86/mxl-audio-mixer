<script setup>
// One node of the routing graph: an input, strip, bus, monitor or published output, with a handle on the
// left (what it takes) and on the right (what it gives).
import { Handle, Position } from "@vue-flow/core";

const props = defineProps({ data: { type: Object, required: true } });
const KIND = { input: "input", strip: "strip", bus: "bus", monitor: "monitor", output: "MXL output" };
</script>

<template>
  <div class="gnode" :class="[`k-${props.data.kind}`, { muted: props.data.muted }]" :style="{ '--colour': props.data.colour || 'var(--border)' }" :data-gnode="props.data.id">
    <Handle v-if="props.data.in" type="target" :position="Position.Left" />
    <div class="gname">{{ props.data.label }}</div>
    <div class="gmeta">{{ KIND[props.data.kind] }}<template v-if="props.data.format"> · {{ props.data.format }}</template><template v-if="props.data.note"> · {{ props.data.note }}</template></div>
    <Handle v-if="props.data.out" type="source" :position="Position.Right" />
  </div>
</template>
