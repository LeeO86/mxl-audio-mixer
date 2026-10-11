<script setup>
// Routing (§12.3): the send matrix (sources × buses; tap an empty cell to send at 0 dB from the source's
// output, tap a send to edit it) and the published outputs (an MXL flow and an NMOS sender each).
import { computed, ref } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import Pot from "./Pot.vue";
import Pill from "./Pill.vue";
import { TAPS, tapLabel } from "../nodes.js";
import { api, dbValue, fmtDb } from "../api.js";
import { act, buses, live, sendOf, sends, signalOf, signals } from "../store.js";

const editing = ref(null); // { from, to }
const current = computed(() => (editing.value ? sendOf(editing.value.from, editing.value.to) : null));
const isPre = (s) => typeof s.tap === "string" && !["post-fader", "post-pan"].includes(s.tap);

async function cell(from, to) {
  if (sendOf(from, to)) {
    editing.value = { from, to };
    return;
  }
  if (await act(() => api.put(`/api/v1/sends/${from}/${to}`))) editing.value = { from, to };
}
const put = (body) => act(() => api.put(`/api/v1/sends/${editing.value.from}/${editing.value.to}`, body));
async function remove() {
  const { from, to } = editing.value;
  if (await act(() => api.del(`/api/v1/sends/${from}/${to}`))) editing.value = null;
}
const tapValue = computed(() => (typeof current.value?.tap === "object" ? `after:${current.value.tap.after}` : current.value?.tap));
const afterOptions = computed(() => (signalOf(editing.value?.from)?.chain || []).map((n) => [`after:${n.id}`, `after ${n.id}`]));
function setTap(v) {
  put({ tap: v.startsWith("after:") ? { after: v.slice(6) } : v });
}
const gain = computed(() => {
  const v = dbValue(current.value?.gain_db);
  return Number.isFinite(v) ? v : -80;
});

const outputs = computed(() => live.status?.outputs || []);
const outputOf = (id) => outputs.value.find((o) => o.signal === id);
const publish = (id) => act(() => api.put(`/api/v1/outputs/${id}`), `${signalOf(id)?.name || id} published.`);
const unpublish = (id) => act(() => api.del(`/api/v1/outputs/${id}`));
</script>

<template>
  <div class="panel">
    <h3>Sends</h3>
    <div v-if="!buses.length" class="empty">No buses yet: add one on the console.</div>
    <div v-else style="overflow-x: auto">
      <table class="matrix">
        <thead>
          <tr>
            <th></th>
            <th v-for="b in buses" :key="b.id" class="col" :title="b.id">{{ b.name }}</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="s in signals" :key="s.id">
            <th :title="s.id" style="text-align: right">{{ s.name }}</th>
            <td v-for="b in buses" :key="b.id" :class="{ self: s.id === b.id }">
              <button
                v-if="s.id !== b.id"
                :class="{ on: sendOf(s.id, b.id), muted: sendOf(s.id, b.id)?.mute, pre: sendOf(s.id, b.id) && isPre(sendOf(s.id, b.id)) }"
                :aria-label="`${s.name} to ${b.name}`"
                :aria-pressed="!!sendOf(s.id, b.id)"
                :title="sendOf(s.id, b.id) ? `${tapLabel(sendOf(s.id, b.id).tap)}, ${fmtDb(sendOf(s.id, b.id).gain_db)} dB` : 'send at 0 dB'"
                @click="cell(s.id, b.id)"
              >
                {{ sendOf(s.id, b.id) ? fmtDb(sendOf(s.id, b.id).gain_db) : "" }}
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <p class="note">{{ sends.length }} sends. Blue: post-fader; dark blue: pre-fader or a chain tap; struck through: muted. A loop is refused unless one of its sends has a block delay.</p>
  </div>

  <div v-if="current" class="panel sendedit">
    <h3>
      {{ signalOf(current.from)?.name }} → {{ signalOf(current.to)?.name }}
      <span class="spacer"></span>
      <button class="btn small secondary" @click="editing = null">Close</button>
    </h3>
    <div class="params">
      <Pot label="Gain" unit="dB" :value="gain" :min="-80" :max="10" :def="0" :fine="0.5" @change="(v) => put({ gain_db: v <= -80 ? '-inf' : v })" />
      <div>
        <label>Tap</label>
        <select :value="tapValue" aria-label="Tap" @change="setTap($event.target.value)">
          <option v-for="[v, l] in TAPS" :key="v" :value="v">{{ l }}</option>
          <option v-for="[v, l] in afterOptions" :key="v" :value="v">{{ l }}</option>
        </select>
        <label class="check"><input type="checkbox" :checked="current.mute" @change="put({ mute: $event.target.checked })" /> Mute</label>
        <label class="check"><input type="checkbox" :checked="current.phase" @change="put({ phase: $event.target.checked })" /> Phase invert</label>
        <label class="check" title="this send reads the source's previous block: one block of delay, allowed in loops">
          <input type="checkbox" :checked="current.block_delay" @change="put({ block_delay: $event.target.checked })" /> Block delay (feedback)
        </label>
      </div>
    </div>
    <div class="actions"><ConfirmButton label="Remove send" @confirm="remove" /></div>
  </div>

  <div class="panel">
    <h3>Published outputs</h3>
    <table>
      <thead>
        <tr><th>Signal</th><th>Format</th><th>State</th><th class="num">Latency ms</th><th class="num">Commit lag ms</th><th>MXL flow</th><th></th></tr>
      </thead>
      <tbody>
        <tr v-for="s in signals" :key="s.id" :class="{ dim: !outputOf(s.id) }">
          <td><strong>{{ s.name }}</strong> <span class="muted small">{{ s.id }}</span></td>
          <td>{{ outputOf(s.id)?.format || s.format }}</td>
          <td>
            <Pill v-if="outputOf(s.id)" :text="outputOf(s.id).errors ? `${outputOf(s.id).errors} errors` : 'published'" :kind="outputOf(s.id).errors ? 'bad' : 'ok'" />
            <span v-else class="muted">–</span>
          </td>
          <td class="num">{{ outputOf(s.id) ? outputOf(s.id).latency_ms.toFixed(2) : "–" }}</td>
          <td class="num">{{ outputOf(s.id) ? outputOf(s.id).commit_lag_ms.toFixed(3) : "–" }}</td>
          <td><code v-if="outputOf(s.id)" :title="outputOf(s.id).mxl_flow_id">{{ outputOf(s.id).mxl_flow_id.slice(0, 8) }}</code></td>
          <td style="text-align: right">
            <ConfirmButton v-if="outputOf(s.id)" label="Unpublish" small @confirm="unpublish(s.id)" />
            <button v-else class="btn small" @click="publish(s.id)">Publish</button>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
