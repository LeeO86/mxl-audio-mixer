<script setup>
// NMOS (§3, §12.3): the node, the inputs (one IS-05 receiver each: routing state, flow, arrival lag,
// delay) and the published outputs (one IS-05 sender each). Routing is done by a controller through
// IS-05 (or in the Graph); here inputs get a label (their alias) and their extra delay. Under the label:
// the name of what is connected (the IS-04 sender label, else the MXL flow's, C2).
import { computed, onMounted, onUnmounted, ref } from "vue";
import Pill from "./Pill.vue";
import { api, shortId } from "../api.js";
import { act, live, signalOf, strips } from "../store.js";

const tally = ref(null);
let timer = 0;
async function loadTally() {
  try {
    tally.value = await api.get("/api/v1/tally");
  } catch {
    /* the banner tells */
  }
}
onMounted(() => {
  loadTally();
  timer = setInterval(loadTally, 1000);
});
onUnmounted(() => clearInterval(timer));
const stripsOf = (index) =>
  strips.value
    .filter((s) => s.tally_index === index)
    .map((s) => s.name + (s.afv?.enabled ? " (AFV)" : ""))
    .join(", ");

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
    <h3>Tally (TSL UMD 5.0)</h3>
    <template v-if="tally?.enabled">
      <dl class="kv">
        <dt>Receiver</dt>
        <dd>udp/{{ tally.udp_port }}, tcp/{{ tally.tcp_port }}; screen {{ tally.screen ?? "any" }}; {{ Object.keys(tally.map).length ? `map ${Object.entries(tally.map).map(([d, i]) => `${d}→${i}`).join(", ")}` : "display index = tally index" }}</dd>
        <dt>Packets</dt>
        <dd>{{ tally.packets }} ({{ tally.errors }} not readable); last {{ tally.last_packet_age_s === null ? "never" : `${tally.last_packet_age_s.toFixed(1)} s ago` }}</dd>
      </dl>
      <table style="margin-top: 0.5rem">
        <thead><tr><th class="num">Index</th><th>Label</th><th>On air</th><th>Preview</th><th class="num">Screen</th><th class="num">Display</th><th>Strips</th><th class="num">Age s</th></tr></thead>
        <tbody>
          <tr v-for="(l, i) in tally.lamps" :key="i">
            <td class="num">{{ i }}</td>
            <td>{{ l.label || "–" }}</td>
            <td><span class="lamp" :class="l.lh ? `c${l.lh}` : ''"></span></td>
            <td><span class="lamp" :class="l.rh ? `c${l.rh}` : ''"></span></td>
            <td class="num">{{ l.screen }}</td>
            <td class="num">{{ l.display }}</td>
            <td>{{ stripsOf(Number(i)) || "–" }}</td>
            <td class="num">{{ l.age_s.toFixed(1) }}</td>
          </tr>
          <tr v-if="!Object.keys(tally.lamps).length"><td colspan="8" class="empty">No tally received yet.</td></tr>
        </tbody>
      </table>
    </template>
    <p v-else class="note" style="margin: 0">Off. With <code>TSL_ENABLE=true</code> the mixer receives TSL UMD 5.0 on <code>TSL_UDP_PORT</code> and <code>TSL_TCP_PORT</code>; strips with a tally index can follow it (audio follows video, under Processing).</p>
  </div>

  <div class="panel">
    <h3>Inputs (receivers)</h3>
    <table>
      <thead>
        <tr>
          <th>Input</th><th>State</th><th>Flow</th><th class="num">Channels</th><th class="num" title="arrival lag p50 / p99">Lag ms</th>
          <th class="num" title="read offset + extra delay">Delay ms</th><th>Extra delay</th><th class="num" title="the source had not written the samples yet">Late</th><th class="num" title="the ring had already overwritten the samples">Overrun</th><th class="num">Missing</th><th></th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="i in inputs" :key="i.input" :class="{ dim: i.state === 'unrouted' }">
          <td>
            <strong>{{ i.input }}</strong>
            <input v-if="editing === i.input" v-model="form.label" aria-label="Label" style="width: 10rem" />
            <span v-else class="muted small"> {{ i.label }}</span>
            <div v-if="i.connected_label" class="muted small" :title="i.sender_id ? `sender ${i.sender_id}` : 'the MXL flow\'s label'">⇠ {{ i.connected_label }}</div>
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
          <td class="num">{{ i.overrun_samples }}</td>
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
    <p class="note">Route inputs with an NMOS controller (IS-05). "auto" raises the extra delay when blocks arrive late; a larger jump is proposed and applied here. Late is a source that had not written the samples yet. Overrun is samples the ring had already overwritten. Missing counts both, and any other failed read.</p>
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
