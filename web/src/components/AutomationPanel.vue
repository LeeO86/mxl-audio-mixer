<script setup>
// The control engine (§8): gain-sharing auto-mixes over strips (weight, bypass, last-mic-hold, VAD
// weighting), ducks (keys, targets, depth, attack, hold, release) and the voice activity detection of
// every analysed signal, each with its live state.
import { computed, ref } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import { api, dbValue, fmtSigned } from "../api.js";
import { act, autoGain, automixOf, automixes, buses, ducks, live, patchAutomix as patchAm, patchDuck as patchDk, signalOf, strips, vadSignals } from "../store.js";

const emit = defineEmits(["close"]);
const nameOf = (id) => signalOf(id)?.name || id;
const fmtGain = (id) => {
  const g = autoGain(id);
  return g === undefined ? "" : Number.isFinite(g) ? fmtSigned(g) : "−∞";
};

// Auto-mixes.
const amForm = ref({ id: "", name: "" });
async function addAutomix() {
  const body = {};
  if (amForm.value.id.trim()) body.id = amForm.value.id.trim();
  if (amForm.value.name.trim()) body.name = amForm.value.name.trim();
  if (await act(() => api.post("/api/v1/automixes", body), "Auto-mix added.")) amForm.value = { id: "", name: "" };
}
const patchAutomix = (a, body) => patchAm(a.id, body);
function setMember(a, id, on) {
  const members = on ? [...a.members, { signal: id, weight_db: 0, bypass: false }] : a.members.filter((m) => m.signal !== id);
  patchAutomix(a, { members });
}
function setMemberField(a, id, field, value) {
  patchAutomix(a, { members: a.members.map((m) => (m.signal === id ? { ...m, [field]: value } : m)) });
}
const inOther = (a, id) => {
  const other = automixOf(id);
  return other && other.id !== a.id;
};

// Ducks.
const duckForm = ref({ id: "", name: "" });
async function addDuck() {
  const body = {};
  if (duckForm.value.id.trim()) body.id = duckForm.value.id.trim();
  if (duckForm.value.name.trim()) body.name = duckForm.value.name.trim();
  if (await act(() => api.post("/api/v1/ducks", body), "Duck added.")) duckForm.value = { id: "", name: "" };
}
const patchDuck = (d, body) => patchDk(d.id, body);
const toggle = (list, id, on) => (on ? [...list, id] : list.filter((x) => x !== id));
const duckActive = (d) => d.enabled && d.keys.some((k) => live.vad[k]?.active);
const depthValue = (d) => {
  const v = dbValue(d.depth_db);
  return Number.isFinite(v) ? v : -120;
};
const setDepth = (d, v) => patchDuck(d, { depth_db: Number(v) <= -120 ? "-inf" : Number(v) });

// Voice detection.
const own = computed(() => Object.fromEntries((live.mix?.vad || []).map((v) => [v.signal, v])));
const DEFAULTS = { threshold: 0.5, hold_ms: 300, min_level_dbfs: -60 };
const settingsOf = (id) => own.value[id] || { signal: id, ...DEFAULTS };
const putVad = (id, body) => act(() => api.put(`/api/v1/vad/${id}`, body));
const analysable = computed(() => [...strips.value, ...buses.value].filter((x) => !vadSignals.value.includes(x.id)));
const addVadSignal = ref("");
async function addVad() {
  if (!addVadSignal.value) return;
  if (await act(() => api.put(`/api/v1/vad/${addVadSignal.value}`, {}), "Voice detection on.")) addVadSignal.value = "";
}
</script>

<template>
  <div class="panel groups automation">
    <h3>Auto-mix, ducking and voice detection <span class="spacer"></span><button class="btn small secondary" @click="emit('close')">Close</button></h3>
    <div class="grid two">
      <div>
        <div class="caption">Auto-mixes (gain sharing)</div>
        <div v-for="a in automixes" :key="a.id" class="groupedit" :data-automix-edit="a.id">
          <div class="row">
            <input :value="a.name" aria-label="Auto-mix name" @change="patchAutomix(a, { name: $event.target.value })" />
            <label class="check"><input type="checkbox" :checked="a.enabled" @change="patchAutomix(a, { enabled: $event.target.checked })" /> on</label>
            <label class="check" title="while nobody speaks, the gains stay"><input type="checkbox" :checked="a.last_mic_hold" @change="patchAutomix(a, { last_mic_hold: $event.target.checked })" /> last-mic hold</label>
            <label class="check" title="a member without speech counts 10 dB less"><input type="checkbox" :checked="a.vad_weighting" @change="patchAutomix(a, { vad_weighting: $event.target.checked })" /> VAD weighting</label>
            <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/automixes/${a.id}`))" />
          </div>
          <table v-if="a.members.length" class="amtable">
            <thead>
              <tr><th>Member</th><th>Weight dB</th><th>Bypass</th><th>Gain</th></tr>
            </thead>
            <tbody>
              <tr v-for="m in a.members" :key="m.signal" :data-member="m.signal">
                <td><span class="vad" :class="{ on: live.vad[m.signal]?.active }">●</span> {{ nameOf(m.signal) }}</td>
                <td><input type="number" min="-30" max="10" step="1" :value="m.weight_db" :aria-label="`${nameOf(m.signal)} weight`" @change="setMemberField(a, m.signal, 'weight_db', Number($event.target.value))" /></td>
                <td><input type="checkbox" :checked="m.bypass" :aria-label="`${nameOf(m.signal)} bypass`" @change="setMemberField(a, m.signal, 'bypass', $event.target.checked)" /></td>
                <td class="num" :data-gain="m.signal">{{ fmtGain(m.signal) }}</td>
              </tr>
            </tbody>
          </table>
          <div class="members">
            <label v-for="x in strips" :key="x.id" class="check" :class="{ muted: inOther(a, x.id) }"
              ><input type="checkbox" :checked="a.members.some((m) => m.signal === x.id)" :disabled="inOther(a, x.id)" :aria-label="`${x.name} in ${a.name}`" @change="setMember(a, x.id, $event.target.checked)" /> {{ x.name }}</label
            >
          </div>
        </div>
        <div class="row">
          <input v-model="amForm.id" placeholder="id (automatic)" aria-label="New auto-mix id" />
          <input v-model="amForm.name" placeholder="name" aria-label="New auto-mix name" />
          <button class="btn small" @click="addAutomix">+ Auto-mix</button>
        </div>

        <div class="caption" style="margin-top: 0.8rem">Voice detection</div>
        <table v-if="vadSignals.length" class="amtable vadtable">
          <thead>
            <tr><th>Signal</th><th>Speech</th><th title="speech starts above this probability">Threshold</th><th>Hold ms</th><th title="below this frame level nothing is speech">Min dBFS</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="id in vadSignals" :key="id" :data-vad-row="id">
              <td>{{ nameOf(id) }}</td>
              <td>
                <span class="prob" :title="`probability ${(live.vad[id]?.probability ?? 0).toFixed(2)}`"
                  ><span :class="{ on: live.vad[id]?.active }" :style="{ width: `${Math.round((live.vad[id]?.probability ?? 0) * 100)}%` }"></span
                ></span>
              </td>
              <td><input type="number" min="0.05" max="0.95" step="0.05" :value="settingsOf(id).threshold" :aria-label="`${nameOf(id)} threshold`" @change="putVad(id, { threshold: Number($event.target.value) })" /></td>
              <td><input type="number" min="0" max="10000" step="50" :value="settingsOf(id).hold_ms" :aria-label="`${nameOf(id)} hold`" @change="putVad(id, { hold_ms: Number($event.target.value) })" /></td>
              <td><input type="number" min="-90" max="0" step="1" :value="settingsOf(id).min_level_dbfs" :aria-label="`${nameOf(id)} minimum level`" @change="putVad(id, { min_level_dbfs: Number($event.target.value) })" /></td>
              <td><button v-if="own[id]" class="btn small secondary" title="back to the defaults (or off, unless an auto-mix or duck needs it)" @click="act(() => api.del(`/api/v1/vad/${id}`))">Reset</button></td>
            </tr>
          </tbody>
        </table>
        <div class="row">
          <select v-model="addVadSignal" aria-label="Signal to analyse">
            <option value="">signal…</option>
            <option v-for="x in analysable" :key="x.id" :value="x.id">{{ x.name }}</option>
          </select>
          <button class="btn small" :disabled="!addVadSignal" @click="addVad">+ Voice detection</button>
        </div>
      </div>

      <div>
        <div class="caption">Ducking by speech</div>
        <div v-for="d in ducks" :key="d.id" class="groupedit" :data-duck-edit="d.id">
          <div class="row">
            <input :value="d.name" aria-label="Duck name" @change="patchDuck(d, { name: $event.target.value })" />
            <label class="check"><input type="checkbox" :checked="d.enabled" @change="patchDuck(d, { enabled: $event.target.checked })" /> on</label>
            <span class="duckstate" :class="{ on: duckActive(d) }" :data-duck-active="duckActive(d)">{{ duckActive(d) ? "speech" : "—" }}</span>
            <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/ducks/${d.id}`))" />
          </div>
          <div class="row duckparams">
            <label>Depth <input type="number" min="-120" max="0" step="1" :value="depthValue(d)" :aria-label="`${d.name} depth`" @change="setDepth(d, $event.target.value)" /> dB</label>
            <label>Attack <input type="number" min="0" max="10000" step="10" :value="d.attack_ms" :aria-label="`${d.name} attack`" @change="patchDuck(d, { attack_ms: Number($event.target.value) })" /> ms</label>
            <label>Hold <input type="number" min="0" max="10000" step="50" :value="d.hold_ms" :aria-label="`${d.name} hold`" @change="patchDuck(d, { hold_ms: Number($event.target.value) })" /> ms</label>
            <label>Release <input type="number" min="0" max="10000" step="50" :value="d.release_ms" :aria-label="`${d.name} release`" @change="patchDuck(d, { release_ms: Number($event.target.value) })" /> ms</label>
          </div>
          <div class="muted small">Keys (their speech ducks)</div>
          <div class="members">
            <label v-for="x in [...strips, ...buses]" :key="x.id" class="check" :class="{ muted: d.targets.includes(x.id) }"
              ><input type="checkbox" :checked="d.keys.includes(x.id)" :disabled="d.targets.includes(x.id)" :aria-label="`${x.name} keys ${d.name}`" @change="patchDuck(d, { keys: toggle(d.keys, x.id, $event.target.checked) })" /> {{ x.name }}</label
            >
          </div>
          <div class="muted small">Targets (ducked)</div>
          <div class="members">
            <label v-for="x in [...strips, ...buses]" :key="x.id" class="check" :class="{ muted: d.keys.includes(x.id) }"
              ><input type="checkbox" :checked="d.targets.includes(x.id)" :disabled="d.keys.includes(x.id)" :aria-label="`${x.name} ducked by ${d.name}`" @change="patchDuck(d, { targets: toggle(d.targets, x.id, $event.target.checked) })" />
              {{ x.name }}<span v-if="d.targets.includes(x.id)" class="num muted"> {{ fmtGain(x.id) }}</span></label
            >
          </div>
        </div>
        <div class="row">
          <input v-model="duckForm.id" placeholder="id (automatic)" aria-label="New duck id" />
          <input v-model="duckForm.name" placeholder="name" aria-label="New duck name" />
          <button class="btn small" @click="addDuck">+ Duck</button>
        </div>
      </div>
    </div>
  </div>
</template>
