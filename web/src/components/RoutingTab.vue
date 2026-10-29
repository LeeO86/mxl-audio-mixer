<script setup>
// Routing (§12.3): the send matrix (rows: sources, columns: destination buses; a click on an empty cell
// marks it yellow, a second click on it within 3 s sends at 0 dB from the source's output, a click
// elsewhere or the 3 s cancel; a click on a send edits it), the virtual outputs (C7: fixed MXL flows and
// NMOS senders the designer wires, MIXER_OUTPUT_FLOWS; pick what feeds each) and the published outputs
// (an MXL flow and an NMOS sender each, made here with Publish; a strip as its post-fader direct out).
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import Pot from "./Pot.vue";
import Pill from "./Pill.vue";
import { tapLabel } from "../nodes.js";
import { loadTaps, tapOptions } from "../taps.js";
import { api, copyText, dbValue, fmtDb } from "../api.js";
import { act, buses, isBus, isSubgroup, live, patchSend, sendOf, sends, signalOf, signals } from "../store.js";
import { setVoutSource, sourceChoices, sourceLabel, vouts } from "../vouts.js";

const editing = ref(null); // { from, to }
const current = computed(() => (editing.value ? sendOf(editing.value.from, editing.value.to) : null));
const isPre = (s) => typeof s.tap === "string" && !["post-fader", "post-pan"].includes(s.tap);

// A new send wants a second click on the same cell (T2).
const armed = ref(""); // "<from>|<to>"
let disarm = 0;
const cellKey = (from, to) => `${from}|${to}`;
function arm(key) {
  clearTimeout(disarm);
  armed.value = key;
  if (key) disarm = setTimeout(() => (armed.value = ""), 3000);
}
// A click anywhere but the marked cell cancels it.
function elsewhere(ev) {
  if (armed.value && ev.target.closest?.("[data-cell]")?.dataset.cell !== armed.value) arm("");
}
onMounted(() => window.addEventListener("pointerdown", elsewhere, true));
onUnmounted(() => {
  window.removeEventListener("pointerdown", elsewhere, true);
  clearTimeout(disarm);
});

async function cell(from, to) {
  if (sendOf(from, to)) {
    arm("");
    editing.value = { from, to };
    return;
  }
  const key = cellKey(from, to);
  if (armed.value !== key) return arm(key);
  arm("");
  if (await act(() => api.put(`/api/v1/sends/${encodeURIComponent(from)}/${encodeURIComponent(to)}`))) editing.value = { from, to };
}
const cellTitle = (s, b) => {
  const x = sendOf(s.id, b.id);
  if (x) return `${s.name} → ${b.name}: ${tapLabel(x.tap)}, ${fmtDb(x.gain_db)} dB`;
  return armed.value === cellKey(s.id, b.id) ? `click again: send ${s.name} → ${b.name} at 0.0 dB` : `${s.name} → ${b.name}: click twice to send at 0.0 dB`;
};
const put = (body) => patchSend(editing.value.from, editing.value.to, body);
async function remove() {
  const { from, to } = editing.value;
  if (await act(() => api.del(`/api/v1/sends/${encodeURIComponent(from)}/${encodeURIComponent(to)}`))) editing.value = null;
}
const tapValue = computed(() => (typeof current.value?.tap === "object" ? `after:${current.value.tap.after}` : current.value?.tap));
// Only the taps the source has (G1), from the server.
watch(() => [editing.value?.from, live.revision], () => editing.value && loadTaps(), { immediate: true });
const tapChoices = computed(() => (current.value ? tapOptions(current.value.from, tapValue.value) : []));
function setTap(v) {
  put({ tap: v.startsWith("after:") ? { after: v.slice(6) } : v });
}
const gain = computed(() => {
  const v = dbValue(current.value?.gain_db);
  return Number.isFinite(v) ? v : -80;
});

const outputs = computed(() => (live.status?.outputs || []).filter((o) => o.virtual_output == null));
// A virtual output's source as its select key: the source without the tap or format (set in the Graph).
const voutKey = (v) => {
  const s = v.source;
  if (!s) return "";
  return JSON.stringify("signal" in s ? { signal: s.signal } : "monitor" in s ? { monitor: s.monitor } : { input: s.input });
};
const pickVout = (v, key) => setVoutSource(v.slot, key ? JSON.parse(key) : null);
const outputOf = (id) => outputs.value.find((o) => o.signal === id);
const publish = (id) => act(() => api.put(`/api/v1/outputs/${encodeURIComponent(id)}`), `${signalOf(id)?.name || id} published.`);
const unpublish = (id) => act(() => api.del(`/api/v1/outputs/${encodeURIComponent(id)}`));
async function copyFlow(o) {
  if (await copyText(o.mxl_flow_id)) live.notice = `MXL flow id of ${signalOf(o.signal)?.name || o.signal} copied: ${o.mxl_flow_id}`;
}
</script>

<template>
  <div class="panel">
    <h3>Sends</h3>
    <div v-if="!buses.length" class="empty">No buses yet: add one on the console.</div>
    <div v-else style="overflow-x: auto">
      <table class="matrix">
        <thead>
          <tr>
            <th class="axis corner" rowspan="2"><span>From (source) ↓</span><span>To (bus) →</span></th>
            <th class="axis" :colspan="buses.length">To: destination buses →</th>
          </tr>
          <tr>
            <th v-for="b in buses" :key="b.id" class="col" :title="`to ${b.name} (${b.id})`">{{ b.name }}</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="s in signals" :key="s.id">
            <th :title="`from ${s.name} (${s.id})`" style="text-align: right">{{ s.name }} <span class="muted">→</span></th>
            <td v-for="b in buses" :key="b.id" :class="{ self: s.id === b.id }">
              <button
                v-if="s.id !== b.id"
                :data-cell="cellKey(s.id, b.id)"
                :class="{ on: sendOf(s.id, b.id), muted: sendOf(s.id, b.id)?.mute, pre: sendOf(s.id, b.id) && isPre(sendOf(s.id, b.id)), armed: armed === cellKey(s.id, b.id) }"
                :aria-label="`${s.name} to ${b.name}`"
                :aria-pressed="!!sendOf(s.id, b.id)"
                :title="cellTitle(s, b)"
                @click="cell(s.id, b.id)"
              >
                {{ sendOf(s.id, b.id) ? fmtDb(sendOf(s.id, b.id).gain_db) : armed === cellKey(s.id, b.id) ? "0.0?" : "" }}
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <p class="note">
      {{ sends.length }} sends. Rows send to columns. A new send: click an empty cell (yellow), then click it again within 3 s. Blue: post-fader; dark blue: pre-fader or a chain tap; struck through: muted. A loop is refused unless one of its sends has a block delay.
    </p>
  </div>

  <div v-if="current" class="panel sendedit">
    <h3>
      {{ signalOf(current.from)?.name }} → {{ signalOf(current.to)?.name }}
      <span class="spacer"></span>
      <button class="btn small secondary" @click="editing = null">Close</button>
    </h3>
    <div class="params">
      <p v-if="isSubgroup(current.to)" class="note" data-subgroup-lock>Into a subgroup: post-fader at 0 dB, locked (C12).</p>
      <Pot v-else label="Gain" unit="dB" :value="gain" :min="-80" :max="10" :def="0" :fine="0.5" @change="(v) => put({ gain_db: v <= -80 ? '-inf' : v })" />
      <div>
        <template v-if="!isSubgroup(current.to)">
          <label>Tap</label>
          <select :value="tapValue" aria-label="Tap" @change="setTap($event.target.value)">
            <option v-for="[v, l] in tapChoices" :key="v" :value="v">{{ l }}</option>
          </select>
        </template>
        <label class="check"><input type="checkbox" :checked="current.mute" @change="put({ mute: $event.target.checked })" /> Mute</label>
        <label class="check"><input type="checkbox" :checked="current.phase" @change="put({ phase: $event.target.checked })" /> Phase invert</label>
        <label class="check" title="this send reads the source's previous block: one block of delay, allowed in loops">
          <input type="checkbox" :checked="current.block_delay" @change="put({ block_delay: $event.target.checked })" /> Block delay (feedback)
        </label>
      </div>
    </div>
    <div class="actions"><ConfirmButton label="Remove send" @confirm="remove" /></div>
  </div>

  <div v-if="vouts.length" class="panel" data-vouts>
    <h3>Virtual outputs</h3>
    <p class="note" style="margin-top: 0">Fixed MXL flows and NMOS senders (the designer wires them): pick what feeds each. Their format stays; a wider or narrower source is down- or up-mixed. Without a source they send silence.</p>
    <table>
      <thead>
        <tr><th>Output</th><th>Format</th><th>Source</th><th>State</th><th class="num">Commit lag ms</th><th>MXL flow</th></tr>
      </thead>
      <tbody>
        <tr v-for="v in vouts" :key="v.slot" :class="{ dim: !v.source }" :data-vout="v.slot">
          <td><strong>{{ v.label }}</strong> <span class="muted small">virtual {{ v.slot }}</span></td>
          <td>{{ v.format }}</td>
          <td>
            <select :value="voutKey(v)" :aria-label="`Source of ${v.label}`" :title="sourceLabel(v.source)" @change="pickVout(v, $event.target.value)">
              <option value="">– silence –</option>
              <option v-for="[k, , l] in sourceChoices" :key="k" :value="k">{{ l }}</option>
            </select>
          </td>
          <td><Pill :text="!v.running ? 'not running' : v.errors ? `${v.errors} errors` : v.source ? 'on' : 'silent'" :kind="!v.running || v.errors ? 'bad' : v.source ? 'ok' : 'neutral'" /></td>
          <td class="num">{{ v.commit_lag_ms == null ? "–" : v.commit_lag_ms.toFixed(3) }}</td>
          <td>
            <template v-if="v.mxl_flow_id">
              <code :title="v.mxl_flow_id">{{ v.mxl_flow_id.slice(0, 8) }}</code>
              <button class="btn small secondary copy" title="Copy the MXL flow id" :aria-label="`Copy the MXL flow id of ${v.label}`" @click="copyFlow({ ...v, signal: v.label })">⧉</button>
            </template>
          </td>
        </tr>
      </tbody>
    </table>
  </div>

  <div class="panel">
    <h3>Published outputs</h3>
    <p class="note" style="margin-top: 0">A bus is published post-fader; a strip as its direct out (post-fader, before the pan).</p>
    <table>
      <thead>
        <tr><th>Signal</th><th>Format</th><th>State</th><th class="num">Latency ms</th><th class="num">Commit lag ms</th><th>MXL flow</th><th></th></tr>
      </thead>
      <tbody>
        <tr v-for="s in signals" :key="s.id" :class="{ dim: !outputOf(s.id) }">
          <td>
            <strong>{{ s.name }}</strong> <span class="muted small">{{ s.id }}</span>
            <span v-if="!isBus(s.id)" class="muted small"> · direct out</span>
          </td>
          <td>{{ outputOf(s.id)?.format || s.format }}</td>
          <td>
            <Pill v-if="outputOf(s.id)" :text="outputOf(s.id).errors ? `${outputOf(s.id).errors} errors` : 'published'" :kind="outputOf(s.id).errors ? 'bad' : 'ok'" />
            <span v-else class="muted">–</span>
          </td>
          <td class="num">{{ outputOf(s.id) ? outputOf(s.id).latency_ms.toFixed(2) : "–" }}</td>
          <td class="num">{{ outputOf(s.id) ? outputOf(s.id).commit_lag_ms.toFixed(3) : "–" }}</td>
          <td>
            <template v-if="outputOf(s.id)">
              <code :title="outputOf(s.id).mxl_flow_id">{{ outputOf(s.id).mxl_flow_id.slice(0, 8) }}</code>
              <button class="btn small secondary copy" title="Copy the MXL flow id" :aria-label="`Copy the MXL flow id of ${s.name}`" @click="copyFlow(outputOf(s.id))">⧉</button>
            </template>
          </td>
          <td style="text-align: right">
            <ConfirmButton v-if="outputOf(s.id)" label="Unpublish" small @confirm="unpublish(s.id)" />
            <button v-else class="btn small" @click="publish(s.id)">Publish</button>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
