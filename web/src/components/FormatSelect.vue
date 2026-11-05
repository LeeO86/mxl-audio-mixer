<script setup>
// The channel format of a strip or bus (C10, P3): pick a format, then confirm (press twice). The chain
// stays: nodes that run on any width adapt, nodes that cannot are bypassed and flagged; sends stay and
// map through the down-mix or up-mix; a strip's input patch is extended or trimmed; the change fades
// the signal out and in. What the server reports (bypassed nodes, a kept output layout) shows as a notice.
import { computed, ref, watch } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import { api } from "../api.js";
import { act, isBus, live, signalPath } from "../store.js";
import { FORMATS, channelsOf, unsupported } from "../nodes.js";

// `ids`: with several selected (B1) the change goes to all of them in one request.
const props = defineProps({ signal: { type: Object, required: true }, ids: { type: Array, default: () => [] } });
const picked = ref(props.signal.format);
watch(
  () => [props.signal.id, props.signal.format],
  () => (picked.value = props.signal.format),
);
const changed = computed(() => picked.value !== props.signal.format);
// The nodes that would be bypassed, said before the change.
const flagged = computed(() =>
  changed.value ? (props.signal.chain || []).filter((n) => !n.bypass && unsupported(n, picked.value, !isBus(props.signal.id))).map((n) => n.id) : [],
);

async function apply() {
  const id = props.signal.id;
  const format = picked.value;
  const many = props.ids.length > 1;
  const answer = many
    ? await act(() => api.patch("/api/v1/signals", { signals: Object.fromEntries(props.ids.map((x) => [x, { format }])) }))
    : await act(() => api.patch(signalPath(id), { format }));
  const what = many ? `${props.ids.length} selected are` : `${props.signal.name} is`;
  if (answer?.warnings?.length) live.notice = `${what} ${format} now: ${answer.warnings.join("; ")}.`;
  else if (answer) live.notice = `${what} ${format} now.`;
  else picked.value = props.signal.format;
}
</script>

<template>
  <div class="formatselect">
    <label>Format</label>
    <div class="row tight">
      <select v-model="picked" :aria-label="`${signal.name} format`">
        <option v-for="f in FORMATS" :key="f" :value="f">{{ f }} ({{ channelsOf(f) }} ch)</option>
      </select>
      <ConfirmButton v-if="changed" small :label="ids.length > 1 ? `Change ${ids.length} to ${picked}` : `Change to ${picked}`" @confirm="apply" />
    </div>
    <p v-if="changed" class="note">
      The chain and the sends stay; the signal fades out and in.
      <template v-if="flagged.length"> Bypassed (cannot run on {{ picked }}): {{ flagged.join(", ") }}.</template>
    </p>
  </div>
</template>
