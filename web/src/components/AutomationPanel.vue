<script setup>
// The control engine (§8): auto-levels (target, speech offset, range, ballistics, freeze), gain-sharing
// auto-mixes over strips (weight, bypass, last-mic-hold, VAD weighting), ducks (keys, targets; fixed:
// depth, attack, hold, release; ratio: Com-IS-Assist's control; the channel groups' shares) and the
// voice activity detection of every analysed signal, each with its live state.
import { computed, ref } from "vue";
import AutoLive from "./AutoLive.vue";
import ConfirmButton from "./ConfirmButton.vue";
import NumberField from "./NumberField.vue";
import { api, dbValue } from "../api.js";
import {
  act,
  autolevels,
  automixOf,
  automixes,
  buses,
  ducks,
  live,
  patchAutomix as patchAm,
  patchDuck as patchDk,
  putAutolevel,
  signalOf,
  strips,
  vadSignals,
} from "../store.js";

const emit = defineEmits(["close"]);
const nameOf = (id) => signalOf(id)?.name || id;

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
const depthValue = (d) => {
  const v = dbValue(d.depth_db);
  return Number.isFinite(v) ? v : -120;
};
const setDepth = (d, v) => patchDuck(d, { depth_db: Number(v) <= -120 ? "-inf" : Number(v) });
const GROUPS = [
  ["front", "L/R"],
  ["center", "C/M"],
  ["lfe", "LFE"],
  ["surround", "Surround"],
];

// Auto-levels.
const levelable = computed(() => [...strips.value, ...buses.value].filter((x) => !autolevels.value.some((l) => l.signal === x.id)));
const addLevelSignal = ref("");
async function addLevel() {
  if (!addLevelSignal.value) return;
  if (await act(() => api.put(`/api/v1/autolevels/${addLevelSignal.value}`, {}), "Auto-level on.")) addLevelSignal.value = "";
}

// Voice detection.
const own = computed(() => Object.fromEntries((live.mix?.vad || []).map((v) => [v.signal, v])));
const DEFAULTS = { threshold: 0.5, hold_ms: 800, gate_lufs: -32, detector: "silero" };
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
    <h3>Auto-level, auto-mix, ducking and voice detection <span class="spacer"></span><button class="btn small secondary" @click="emit('close')">Close</button></h3>
    <div class="grid two">
      <div>
        <div class="caption">Auto-mixes (gain sharing)</div>
        <div v-for="a in automixes" :key="a.id" class="groupedit" :data-automix-edit="a.id">
          <div class="row">
            <input :value="a.name" aria-label="Auto-mix name" @change="patchAutomix(a, { name: $event.target.value })" />
            <label class="check"><input type="checkbox" :checked="a.enabled" @change="patchAutomix(a, { enabled: $event.target.checked })" /> on</label>
            <label class="check" title="while nobody speaks, the gains stay"><input type="checkbox" :checked="a.last_mic_hold" @change="patchAutomix(a, { last_mic_hold: $event.target.checked })" /> last-mic hold</label>
            <label class="check" title="a member without speech counts 10 dB less"><input type="checkbox" :checked="a.vad_weighting" @change="patchAutomix(a, { vad_weighting: $event.target.checked })" /> VAD weighting</label>
            <label class="check" title="a snapshot's recall leaves it"><input type="checkbox" :checked="a.recall_safe" @change="patchAutomix(a, { recall_safe: $event.target.checked })" /> recall-safe</label>
            <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/automixes/${a.id}`))" />
          </div>
          <table v-if="a.members.length" class="amtable">
            <thead>
              <tr><th>Member</th><th>Weight dB</th><th>Bypass</th><th>Gain</th></tr>
            </thead>
            <tbody>
              <tr v-for="m in a.members" :key="m.signal" :data-member="m.signal">
                <td><AutoLive kind="dot" :id="m.signal" /> {{ nameOf(m.signal) }}</td>
                <td><NumberField :min="-30" :max="10" :step="1" :value="m.weight_db" :aria-label="`${nameOf(m.signal)} weight`" @change="(v) => setMemberField(a, m.signal, 'weight_db', v)" /></td>
                <td><input type="checkbox" :checked="m.bypass" :aria-label="`${nameOf(m.signal)} bypass`" @change="setMemberField(a, m.signal, 'bypass', $event.target.checked)" /></td>
                <td><AutoLive kind="gain" :id="m.signal" /></td>
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

        <div class="caption" style="margin-top: 0.8rem">Auto-level (speech-aware loudness)</div>
        <table v-if="autolevels.length" class="amtable leveltable">
          <thead>
            <tr>
              <th>Signal</th>
              <th title="the gain and the short-term loudness of the signal (before the gain)">Gain · level</th>
              <th title="short-term loudness the gain rides to">Target LUFS</th>
              <th title="added to the target while the signal speaks (its voice detection)">Speech LU</th>
              <th title="the gain's range">Min / max dB</th>
              <th title="time constants of the gain going down and up">Attack / release ms</th>
              <th title="below this momentary loudness there is no signal: the gain stays">Freeze LUFS</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="l in autolevels" :key="l.signal" :data-level-row="l.signal">
              <td>
                <label class="check"><input type="checkbox" :checked="l.enabled" :aria-label="`${nameOf(l.signal)} auto-level on`" @change="putAutolevel(l.signal, { enabled: $event.target.checked })" /> {{ nameOf(l.signal) }}</label>
              </td>
              <td><AutoLive kind="level" :id="l.signal" /></td>
              <td><NumberField :min="-40" :max="0" :step="1" :value="l.target_lufs" :aria-label="`${nameOf(l.signal)} target`" @change="(v) => putAutolevel(l.signal, { target_lufs: v })" /></td>
              <td><NumberField :min="-10" :max="10" :step="0.5" :value="l.speech_offset_lu" :aria-label="`${nameOf(l.signal)} speech offset`" @change="(v) => putAutolevel(l.signal, { speech_offset_lu: v })" /></td>
              <td>
                <NumberField :min="-24" :max="0" :step="1" :value="l.min_gain_db" :aria-label="`${nameOf(l.signal)} min gain`" @change="(v) => putAutolevel(l.signal, { min_gain_db: v })" />
                <NumberField :min="0" :max="24" :step="1" :value="l.max_gain_db" :aria-label="`${nameOf(l.signal)} max gain`" @change="(v) => putAutolevel(l.signal, { max_gain_db: v })" />
              </td>
              <td>
                <NumberField :min="0" :max="60000" :step="100" :value="l.attack_ms" :aria-label="`${nameOf(l.signal)} attack`" @change="(v) => putAutolevel(l.signal, { attack_ms: v })" />
                <NumberField :min="0" :max="60000" :step="100" :value="l.release_ms" :aria-label="`${nameOf(l.signal)} release`" @change="(v) => putAutolevel(l.signal, { release_ms: v })" />
              </td>
              <td><NumberField :min="-70" :max="0" :step="1" :value="l.freeze_lufs" :aria-label="`${nameOf(l.signal)} freeze`" @change="(v) => putAutolevel(l.signal, { freeze_lufs: v })" /></td>
              <td>
                <label class="check" title="a snapshot's recall leaves it"><input type="checkbox" :checked="l.recall_safe" :aria-label="`${nameOf(l.signal)} auto-level recall-safe`" @change="putAutolevel(l.signal, { recall_safe: $event.target.checked })" /> safe</label>
                <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/autolevels/${l.signal}`))" />
              </td>
            </tr>
          </tbody>
        </table>
        <div class="row">
          <select v-model="addLevelSignal" aria-label="Signal to auto-level">
            <option value="">signal…</option>
            <option v-for="x in levelable" :key="x.id" :value="x.id">{{ x.name }}</option>
          </select>
          <button class="btn small" :disabled="!addLevelSignal" @click="addLevel">+ Auto-level</button>
        </div>

        <div class="caption" style="margin-top: 0.8rem">Voice detection</div>
        <table v-if="vadSignals.length" class="amtable vadtable">
          <thead>
            <tr><th>Signal</th><th>Speech</th><th title="Silero (a small neural network) or the feature detector">Detector</th><th title="speech starts above this probability">Threshold</th><th>Hold ms</th><th title="while the momentary loudness is below this, nothing is speech (crosstalk is quieter than the talker)">Gate LUFS</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="id in vadSignals" :key="id" :data-vad-row="id">
              <td>{{ nameOf(id) }}</td>
              <td><AutoLive kind="probability" :id="id" /></td>
              <td>
                <select :value="settingsOf(id).detector" :aria-label="`${nameOf(id)} detector`" @change="putVad(id, { detector: $event.target.value })">
                  <option value="silero">Silero</option>
                  <option value="features">Features</option>
                </select>
              </td>
              <td><NumberField :min="0.05" :max="0.95" :step="0.05" :value="settingsOf(id).threshold" :aria-label="`${nameOf(id)} threshold`" @change="(v) => putVad(id, { threshold: v })" /></td>
              <td><NumberField :min="0" :max="10000" :step="50" :value="settingsOf(id).hold_ms" :aria-label="`${nameOf(id)} hold`" @change="(v) => putVad(id, { hold_ms: v })" /></td>
              <td><NumberField :min="-70" :max="0" :step="1" :value="settingsOf(id).gate_lufs" :aria-label="`${nameOf(id)} gate`" @change="(v) => putVad(id, { gate_lufs: v })" /></td>
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
            <label class="check" title="a snapshot's recall leaves it"><input type="checkbox" :checked="d.recall_safe" @change="patchDuck(d, { recall_safe: $event.target.checked })" /> recall-safe</label>
            <select :value="d.mode" :aria-label="`${d.name} mode`" title="fixed: down by the depth while a key speaks; ratio: as far down as keeps the keys a ratio above the targets" @change="patchDuck(d, { mode: $event.target.value })">
              <option value="fixed">Fixed depth</option>
              <option value="ratio">Ratio</option>
            </select>
            <AutoLive v-if="d.mode === 'ratio'" kind="ratio" :duck="d" />
            <AutoLive v-else kind="duck" :duck="d" />
            <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/ducks/${d.id}`))" />
          </div>
          <div v-if="d.mode === 'ratio'" class="row duckparams">
            <label title="how far the keys sit above the targets while a key speaks">Ratio <NumberField :min="0" :max="12" :step="1" :value="d.ratio.target_lu" :aria-label="`${d.name} ratio`" @change="(v) => patchDuck(d, { ratio: { target_lu: v } })" /> LU</label>
            <label title="the ratio while the targets speak too">Over-voice <NumberField :min="0" :max="24" :step="1" :value="d.ratio.overvoice_lu" :aria-label="`${d.name} over-voice`" @change="(v) => patchDuck(d, { ratio: { overvoice_lu: v } })" /> LU</label>
            <label>Max <NumberField :min="0" :max="48" :step="1" :value="d.ratio.max_reduction_db" :aria-label="`${d.name} max reduction`" @change="(v) => patchDuck(d, { ratio: { max_reduction_db: v } })" /> dB</label>
            <label title="scales every stage's attack and release">Speed <NumberField :min="0.25" :max="8" :step="0.25" :value="d.ratio.speed" :aria-label="`${d.name} speed`" @change="(v) => patchDuck(d, { ratio: { speed: v } })" /> ×</label>
            <label class="check" title="the targets' speech counts: alone it brings them back fast (interview), with a key's the over-voice ratio applies"
              ><input type="checkbox" :checked="d.ratio.bed_voice" :aria-label="`${d.name} bed voice`" @change="patchDuck(d, { ratio: { bed_voice: $event.target.checked } })" /> bed voice</label
            >
          </div>
          <div v-else class="row duckparams">
            <label>Depth <NumberField :min="-120" :max="0" :step="1" :value="depthValue(d)" :aria-label="`${d.name} depth`" @change="(v) => setDepth(d, v)" /> dB</label>
            <label>Attack <NumberField :min="0" :max="10000" :step="10" :value="d.attack_ms" :aria-label="`${d.name} attack`" @change="(v) => patchDuck(d, { attack_ms: v })" /> ms</label>
            <label>Hold <NumberField :min="0" :max="10000" :step="50" :value="d.hold_ms" :aria-label="`${d.name} hold`" @change="(v) => patchDuck(d, { hold_ms: v })" /> ms</label>
            <label>Release <NumberField :min="0" :max="10000" :step="50" :value="d.release_ms" :aria-label="`${d.name} release`" @change="(v) => patchDuck(d, { release_ms: v })" /> ms</label>
          </div>
          <div class="row duckparams" title="the share of the duck's dB each channel group of a target takes (1: all of it, 0: none)">
            <span class="muted small">Channel share</span>
            <label v-for="[k, label] in GROUPS" :key="k"
              >{{ label }} <NumberField :min="0" :max="1" :step="0.1" :value="d.channel_depth[k]" :aria-label="`${d.name} ${label} share`" @change="(v) => patchDuck(d, { channel_depth: { [k]: v } })"
            /></label>
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
              {{ x.name }}<AutoLive v-if="d.targets.includes(x.id)" kind="gain" :id="x.id" class="muted" /></label
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
