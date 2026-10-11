<script setup>
// NMOS (§3, §12.3): the node, the inputs (one IS-05 receiver each: routing state, flow, arrival lag,
// delay) and the published outputs (one IS-05 sender each). Routing is done by a controller through
// IS-05; here inputs get a label and their extra delay.
import { computed, ref } from "vue";
import Pill from "./Pill.vue";
import { api, shortId } from "../api.js";
import { act, live, signalOf } from "../store.js";

const nmos = computed(() => live.status?.nmos || {});
const inputs = computed(() => live.status?.inputs || []);
const outputs = computed(() => live.status?.outputs || []);
const STATE_KIND = { running: "ok", unrouted: "neutral", waiting: "warn", missing: "bad", error: "bad" };
const ms = (v) => (v === null || v === undefined ? "–" : Number(v).toFixed(2));

const editing = ref(0);
const form = ref({ label: "", delay: "auto" });
function edit(i) {
  editing.value = i.input;
  form.value = { label: i.label, delay: i.extra_delay === "auto" ? "auto" : String(i.extra_delay_ms) };
}
async function save(i) {
  const extra_delay = form.value.delay.trim() === "auto" ? "auto" : Number(form.value.delay);
  if (extra_delay !== "auto" && !Number.isFinite(extra_delay)) {
    live.actionError = "The extra delay is auto or a number of ms.";
    return;
  }
  if (await act(() => api.patch(`/api/v1/inputs/${i.input}`, { label: form.value.label, extra_delay }))) editing.value = 0;
}
const confirm = (i) => act(() => api.post(`/api/v1/inputs/${i.input}/delay/confirm`), `Input ${i.input}: the proposed delay is applied.`);
</script>

<template>
  <div class="panel">
    <h3>Node</h3>
    <dl class="kv">
      <dt>Node id</dt>
      <dd><code>{{ nmos.node_id || "–" }}</code></dd>
      <dt>APIs</dt>
      <dd>
        <a :href="`/x-nmos/node/v1.3/`" target="_blank">IS-04 node</a> · <a :href="`/x-nmos/connection/v1.2/`" target="_blank">IS-05 connection</a> (also on
        {{ nmos.host }}:{{ nmos.port }})
      </dd>
      <dt>Registration</dt>
      <dd><Pill :text="nmos.registry ? (nmos.registered ? 'registered' : 'not registered') : 'no registry configured'" :kind="nmos.registered ? 'ok' : nmos.registry ? 'warn' : 'neutral'" /></dd>
    </dl>
  </div>

  <div class="panel">
    <h3>Inputs (receivers)</h3>
    <table>
      <thead>
        <tr>
          <th>Input</th><th>State</th><th>Flow</th><th class="num">Channels</th><th class="num" title="arrival lag p50 / p99">Lag ms</th>
          <th class="num" title="read offset + extra delay">Delay ms</th><th>Extra delay</th><th class="num">Late</th><th class="num">Missing</th><th></th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="i in inputs" :key="i.input" :class="{ dim: i.state === 'unrouted' }">
          <td>
            <strong>{{ i.input }}</strong>
            <input v-if="editing === i.input" v-model="form.label" aria-label="Label" style="width: 10rem" />
            <span v-else class="muted small"> {{ i.label }}</span>
          </td>
          <td><Pill :text="i.state" :kind="STATE_KIND[i.state] || 'neutral'" /></td>
          <td><code v-if="i.flow" :title="`${i.flow} in ${i.domain}`">{{ shortId(i.flow) }}</code><span v-else class="muted">–</span></td>
          <td class="num">{{ i.channels || "–" }}</td>
          <td class="num">{{ i.arrival_lag_ms ? `${ms(i.arrival_lag_ms.p50)} / ${ms(i.arrival_lag_ms.p99)}` : "–" }}</td>
          <td class="num">{{ ms(i.effective_delay_ms) }}</td>
          <td>
            <input v-if="editing === i.input" v-model="form.delay" aria-label="Extra delay (auto or ms)" style="width: 6rem" />
            <template v-else>{{ i.extra_delay === "auto" ? `auto (${ms(i.extra_delay_ms)})` : `${ms(i.extra_delay_ms)} ms` }}</template>
            <button v-if="i.proposed_delay_ms !== null && i.proposed_delay_ms !== undefined" class="btn small" :title="`raise the extra delay to ${ms(i.proposed_delay_ms)} ms`" @click="confirm(i)">
              Apply {{ ms(i.proposed_delay_ms) }}
            </button>
          </td>
          <td class="num">{{ i.late_samples }}</td>
          <td class="num">{{ i.missing_samples }}</td>
          <td style="text-align: right; white-space: nowrap">
            <template v-if="editing === i.input">
              <button class="btn small secondary" @click="editing = 0">Cancel</button>
              <button class="btn small" @click="save(i)">Save</button>
            </template>
            <button v-else class="btn small secondary" @click="edit(i)">Edit</button>
          </td>
        </tr>
      </tbody>
    </table>
    <p class="note">Route inputs with an NMOS controller (IS-05). "auto" raises the extra delay when blocks arrive late; a larger jump is proposed and applied here.</p>
  </div>

  <div class="panel">
    <h3>Outputs (senders)</h3>
    <table>
      <thead>
        <tr><th>Signal</th><th>Format</th><th>Sender</th><th>MXL flow</th><th class="num">Commit samples</th><th class="num">Commits</th><th class="num">Errors</th></tr>
      </thead>
      <tbody>
        <tr v-for="o in outputs" :key="o.signal">
          <td><strong>{{ o.name || signalOf(o.signal)?.name || o.signal }}</strong> <span class="muted small">{{ o.signal }}</span></td>
          <td>{{ o.format }}</td>
          <td><code :title="o.nmos_sender_id">{{ shortId(o.nmos_sender_id) }}</code></td>
          <td><code :title="o.mxl_flow_id">{{ shortId(o.mxl_flow_id) }}</code></td>
          <td class="num">{{ o.commit_samples }}</td>
          <td class="num">{{ o.commits }}</td>
          <td class="num">{{ o.errors }}</td>
        </tr>
        <tr v-if="!outputs.length"><td colspan="7" class="empty">Nothing published: publish a strip or bus under Routing.</td></tr>
      </tbody>
    </table>
  </div>
</template>
