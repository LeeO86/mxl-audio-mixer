<script setup>
// One block on the logic canvas: a row per port, inputs on the left and outputs on the right, each
// coloured by its type (§11.1). The live value (§11.2 debug view) is the pill at the bottom right. A
// validation issue (D162) outlines the block or rings the port in red, with the message as its title.
// While a cable is being drawn, inputs that cannot take it are dimmed (D158).
import { computed, inject } from "vue";
import { Handle, Position } from "@vue-flow/core";
import { canWire } from "../logicEdit.js";

const props = defineProps({ data: { type: Object, required: true } });
const drawing = inject("logicDrawing"); // ref: { type, from: "source" | "target" } while a cable is drawn
const rows = computed(() => Math.max(props.data.inputs.length, props.data.outputs.length));
const refused = (side, type) => {
  const d = drawing.value;
  if (!d) return false;
  if (side === "target") return d.from === "source" && !canWire(d.type, type);
  return d.from === "target" && !canWire(type, d.type);
};
</script>

<template>
  <div class="gnode lnode" :class="[`k-${props.data.kind}`, { bad: props.data.issues.length, fired: props.data.fired, instance: props.data.instance }]" :data-lnode="props.data.id" :title="props.data.issues.join('\n') || null">
    <div class="gname">
      {{ props.data.label }}<span v-if="props.data.issues.length" class="lbad" :data-issue="props.data.id">!</span>
    </div>
    <div v-for="i in rows" :key="i" class="lrow">
      <span class="lin">
        <template v-if="props.data.inputs[i - 1]">
          <Handle
            :id="props.data.inputs[i - 1].name"
            type="target"
            :position="Position.Left"
            :class="['typed', `t-${props.data.inputs[i - 1].type}`, { bad: props.data.badPorts[props.data.inputs[i - 1].name], nope: refused('target', props.data.inputs[i - 1].type) }]"
            :title="`${props.data.inputs[i - 1].name}: ${props.data.inputs[i - 1].type}${props.data.badPorts[props.data.inputs[i - 1].name] ? ` (${props.data.badPorts[props.data.inputs[i - 1].name]})` : ''}`"
          />
          {{ props.data.inputs[i - 1].name }}
        </template>
      </span>
      <span class="lout">
        <template v-if="props.data.outputs[i - 1]">
          {{ props.data.outputs[i - 1].name }}
          <Handle
            :id="props.data.outputs[i - 1].name"
            type="source"
            :position="Position.Right"
            :class="['typed', `t-${props.data.outputs[i - 1].type}`, { bad: props.data.badPorts[props.data.outputs[i - 1].name], nope: refused('source', props.data.outputs[i - 1].type) }]"
            :title="`${props.data.outputs[i - 1].name}: ${props.data.outputs[i - 1].type}`"
          />
        </template>
      </span>
    </div>
    <div class="lfoot">
      <span class="gmeta">{{ props.data.note }}</span>
      <span v-if="props.data.live !== ''" class="lval" :class="{ on: props.data.on === true, off: props.data.on === false }" :data-live="props.data.id">{{ props.data.live }}</span>
    </div>
  </div>
</template>
