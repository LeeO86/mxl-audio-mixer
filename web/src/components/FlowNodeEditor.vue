<script setup>
// The flow blocks of the processing view (1.6.0): the fader's place (C3), a direct out (C5) and an
// insert (C4). A direct out and an insert send are published outputs: their key, MXL flow id and NMOS
// sender come from GET /api/v1/status (outputs). The insert's return is an input of the mixer: pick the
// input and, if wanted, the MXL flow it reads (the same route the Graph tab sets). Meters out and in,
// the levels (the generic fields of the node), and the measured round trip (no delay compensation).
import { computed, ref } from "vue";
import Meter from "./Meter.vue";
import { FORMATS, outputKey } from "../nodes.js";
import { api } from "../api.js";
import { act, inputName, live, patchNode, signalOf } from "../store.js";

const props = defineProps({
  signal: { type: String, required: true },
  node: { type: Object, required: true },
});
const set = (body) => patchNode(props.signal, props.node.id, body);
const sig = computed(() => signalOf(props.signal));
const key = computed(() => outputKey(props.signal, props.node));
const out = computed(() => (live.status?.outputs || []).find((o) => o.signal === key.value) || null);
const channels = computed(() => ({ mono: 1, stereo: 2, "3.0": 3, "5.1": 6, "7.1": 8, "5.1.4": 10, "7.1.4": 12 })[sig.value?.format] || 1);
const meterOf = (side) => live.meters[`insert:${key.value}:${side}`] || null;
const inputs = computed(() => live.status?.inputs || []);
const returnInput = computed(() => inputs.value.find((i) => i.input === props.node.return_input) || null);
const roundTrip = computed(() => {
  const ms = out.value?.round_trip_ms;
  return typeof ms === "number" ? `${ms.toFixed(1)} ms` : "not measured (needs signal coming back)";
});
const copied = ref(false);
function copyFlow() {
  if (!out.value) return;
  navigator.clipboard?.writeText(out.value.mxl_flow_id).catch(() => {});
  copied.value = true;
  setTimeout(() => (copied.value = false), 1200);
}

// The return's flow: the MXL flows of the domains (as in the Graph tab), routed to the return input.
const flows = ref([]);
const flowOpen = ref(false);
async function loadFlows() {
  flowOpen.value = !flowOpen.value;
  if (!flowOpen.value) return;
  const list = await act(() => api.get("/api/v1/flows"));
  if (Array.isArray(list)) flows.value = list;
}
const flowLabel = (f) => `${f.label || f.flow_id.slice(0, 8)} · ${f.channels} ch${f.own ? " (own)" : ""}`;
async function route(v) {
  const n = props.node.return_input;
  if (!n || !v) return;
  const [domain_id, flow_id] = v.split("|");
  await act(() => api.put(`/api/v1/inputs/${n}/route`, { flow_id, domain_id }), `Input ${n} reads the return.`);
  flowOpen.value = false;
}
</script>

<template>
  <div class="flownode">
    <p v-if="node.type === 'fader'" class="note">The fader stage (fader, audio-follows-video, mute) runs here. Units after it run post-fader; the post-fader tap is the end of the flow, “after fader” is right after it.</p>

    <template v-else-if="node.type === 'direct-out'">
      <div class="params">
        <div>
          <label>Published as</label>
          <div class="row tight">
            <code>{{ out?.name || key }}</code>
            <button v-if="out" class="btn small secondary" :title="`Copy the MXL flow id ${out.mxl_flow_id}`" @click="copyFlow">{{ copied ? "✓" : "⧉ flow" }}</button>
          </div>
          <p class="note">Output key <code>{{ key }}</code>{{ out ? ` · ${out.channels} ch · write delay ${out.write_delay_ms.toFixed(1)} ms` : " · not published (see errors)" }}{{ node.bypass ? " · bypassed: writes silence" : "" }}</p>
        </div>
        <div>
          <label>Layout of the flow</label>
          <select :value="node.format || ''" aria-label="Direct out layout" @change="set({ format: $event.target.value || null })">
            <option value="">The strip's ({{ sig?.format }})</option>
            <option v-for="f in FORMATS" :key="f" :value="f">{{ f }}</option>
          </select>
        </div>
      </div>
    </template>

    <template v-else-if="node.type === 'insert'">
      <div class="row tight insertmeters">
        <div class="side">
          <span class="muted small">Out</span>
          <Meter :signal="`insert:${key}:send`" :values="meterOf('send')" :channels="channels" :marks="false" />
        </div>
        <div class="side">
          <span class="muted small">In</span>
          <Meter :signal="`insert:${key}:return`" :values="meterOf('return')" :channels="channels" :marks="false" />
        </div>
        <div class="params">
          <div>
            <label>Send (MXL flow)</label>
            <div class="row tight">
              <code>{{ out?.name || key }}</code>
              <button v-if="out" class="btn small secondary" :title="`Copy the MXL flow id ${out.mxl_flow_id}`" @click="copyFlow">{{ copied ? "✓" : "⧉ flow" }}</button>
            </div>
          </div>
          <div>
            <label>Return (input)</label>
            <div class="row tight">
              <select :value="node.return_input ?? ''" aria-label="Return input" @change="set({ return_input: $event.target.value ? Number($event.target.value) : null })">
                <option value="">— none (dry)</option>
                <option v-for="i in inputs" :key="i.input" :value="i.input">{{ i.input }}: {{ inputName(i) }}</option>
              </select>
              <label class="muted small">from channel</label>
              <input type="number" min="1" max="64" :value="node.return_channel" aria-label="Return first channel" style="width: 4rem" @change="set({ return_channel: Math.max(1, Math.min(64, Math.round(Number($event.target.value) || 1))) })" />
              <button v-if="node.return_input" class="btn small secondary" @click="loadFlows">Flow…</button>
            </div>
            <select v-if="flowOpen" aria-label="Return flow" @change="route($event.target.value)">
              <option value="">Read which MXL flow on input {{ node.return_input }}?</option>
              <option v-for="f in flows" :key="`${f.domain_id}|${f.flow_id}`" :value="`${f.domain_id}|${f.flow_id}`">{{ flowLabel(f) }}</option>
            </select>
            <p class="note">
              <template v-if="returnInput">Reads {{ returnInput.connected_label || returnInput.flow || "nothing yet" }}.</template>
              Round trip: <span data-round-trip>{{ roundTrip }}</span> (not compensated){{ node.bypass ? " · bypassed: dry" : "" }}
            </p>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.insertmeters {
  align-items: flex-start;
  gap: 0.8rem;
}
.insertmeters {
  flex-wrap: wrap;
}
.side {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.2rem;
}
.insertmeters .params {
  flex: 1 1 16rem;
}
</style>
