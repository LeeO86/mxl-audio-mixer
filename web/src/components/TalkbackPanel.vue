<script setup>
// Talkback (§10.2): pick the mic (a configured one or this browser's microphone), talk to a destination
// (a short press latches, a long one talks while held), to all of them, or listen back to a destination's
// return on the monitors. Below, the mics and destinations of the show.
import { computed, ref } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import Meter from "./Meter.vue";
import Pot from "./Pot.vue";
import Segmented from "./Segmented.vue";
import { api, dbValue } from "../api.js";
import { act, buses, live, room, signalOf, signals } from "../store.js";
import { browserMic, isTalking, listenBack, micAvailable, micOff, micOn, talkButton, talking } from "../talkback.js";

const props = defineProps({
  // Only these destinations (the talkback widget), and no settings.
  only: { type: Array, default: null },
  compact: { type: Boolean, default: false },
  // This mic, without the choice (the talkback widget's `mic`).
  mic: { type: String, default: null },
});

const t = computed(() => room.value.talkback || { mics: [], destinations: [], browser_priority: 0 });
const BROWSER = "__browser";
const MIC_KEY = "mxl-audio-mixer.talkback-mic";
const selected = ref(sessionStorage.getItem(MIC_KEY) || "");
const choice = computed(() => {
  if (props.mic) return props.mic;
  if (selected.value === BROWSER || t.value.mics.some((m) => m.id === selected.value)) return selected.value;
  return t.value.mics[0]?.id || BROWSER;
});
function choose(v) {
  selected.value = v;
  sessionStorage.setItem(MIC_KEY, v);
}
const micId = computed(() => (choice.value === BROWSER ? browserMic.id : choice.value));
const ready = computed(() => !!micId.value && (choice.value !== BROWSER || browserMic.state === "on"));
const destinations = computed(() => t.value.destinations.filter((d) => !props.only || props.only.includes(d.id)));
const meterKey = computed(() => (micId.value ? `talkback:${micId.value}` : ""));

const handlers = new Map();
function down(d, e) {
  if (!ready.value) return;
  e.currentTarget?.setPointerCapture?.(e.pointerId);
  const h = talkButton(micId.value, d.id);
  handlers.set(d.id, h);
  h.down();
}
function up(d) {
  handlers.get(d.id)?.up();
  handlers.delete(d.id);
}
const talksTo = (d) => !!micId.value && isTalking(micId.value, d.id);
const othersTalk = (d) => Object.entries(talking()).some(([m, list]) => m !== micId.value && list.includes(d.id));
const allTalk = computed(() => !!micId.value && destinations.value.length > 0 && destinations.value.every(talksTo));
async function all() {
  if (!ready.value) return;
  const mic = encodeURIComponent(micId.value);
  if (allTalk.value) await act(() => api.del(`/api/v1/talkback/talk/${mic}`));
  else for (const d of destinations.value.filter((x) => !talksTo(x))) await act(() => api.put(`/api/v1/talkback/talk/${mic}/${encodeURIComponent(d.id)}`, {}));
}

const sameSource = (a, b) => !!a && !!b && JSON.stringify(a) === JSON.stringify(b);
const listening = (d) => sameSource(listenBack(), d.return);
const toggleListen = (d) =>
  act(() => (listening(d) ? api.del("/api/v1/listen-back") : api.put("/api/v1/listen-back", { destination: d.id })));
const sourceName = (s) => (!s ? "" : "signal" in s ? signalOf(s.signal)?.name || s.signal : `Input ${s.input}`);

// Settings.
const setMic = (id, body) => act(() => api.patch(`/api/v1/talkback/mics/${encodeURIComponent(id)}`, body));
const setDest = (id, body) => act(() => api.patch(`/api/v1/talkback/destinations/${encodeURIComponent(id)}`, body));
const inputs = computed(() => live.status?.inputs || []);
const addMic = () => act(() => api.post("/api/v1/talkback/mics", { input: inputs.value[0]?.input || 1, channel: 1, name: "Talkback" }), "Talkback mic added.");
const freeBuses = computed(() => buses.value.filter((b) => !t.value.destinations.some((d) => d.bus === b.id)));
const addDest = () => act(() => api.post("/api/v1/talkback/destinations", { bus: freeBuses.value[0].id, name: freeBuses.value[0].name }), "Destination added.");
const returnChoices = computed(() => [
  ...signals.value.map((x) => ({ value: `s:${x.id}`, label: x.name })),
  ...inputs.value.map((i) => ({ value: `i:${i.input}`, label: `Input ${i.input}` })),
]);
const returnValue = (d) => (!d.return ? "" : "signal" in d.return ? `s:${d.return.signal}` : `i:${d.return.input}`);
function setReturn(d, v) {
  setDest(d.id, { return: !v ? null : v.startsWith("s:") ? { signal: v.slice(2) } : { input: Number(v.slice(2)) } });
}
const interruptValue = (d) => {
  const v = dbValue(d.interrupt_db);
  return Number.isFinite(v) ? Math.max(-60, v) : -60;
};
</script>

<template>
  <div class="panel talkback" :class="{ compact }">
    <h3 v-if="!compact">
      Talkback
      <span class="spacer"></span>
      <span v-if="listenBack()" class="pill warn" title="the monitors that follow solo play the return">listen-back: {{ sourceName(listenBack()) }}</span>
    </h3>
    <div class="tbrow">
      <span v-if="mic" class="caption">{{ t.mics.find((m) => m.id === mic)?.name || mic }}</span>
      <select v-else :value="choice" aria-label="Talkback mic" @change="choose($event.target.value)">
        <option v-for="m in t.mics" :key="m.id" :value="m.id">{{ m.name }}</option>
        <option :value="BROWSER" :disabled="!micAvailable()">This browser's microphone{{ micAvailable() ? "" : " (needs HTTPS)" }}</option>
      </select>
      <template v-if="choice === BROWSER">
        <button v-if="browserMic.state === 'off' || browserMic.state === 'error'" class="btn small" :title="browserMic.error || 'switch the microphone on'" @click="micOn">Mic on</button>
        <button v-else class="btn small on-warn" :title="browserMic.state" @click="micOff">{{ browserMic.state === "on" ? "Mic off" : "Starting…" }}</button>
      </template>
      <div v-if="meterKey" class="tbmeter"><Meter :signal="meterKey" :values="live.meters[meterKey]" :channels="1" scale="dbfs" :marks="false" /></div>
    </div>
    <p v-if="choice === BROWSER && browserMic.state === 'error'" class="note bad">{{ browserMic.error }}</p>
    <div class="tbbuttons">
      <button
        v-for="d in destinations"
        :key="d.id"
        class="btn talk"
        :class="{ on: talksTo(d), others: !talksTo(d) && othersTalk(d) }"
        :disabled="!ready"
        :aria-pressed="talksTo(d)"
        :title="`${d.mode === 'interrupt' ? 'interrupts' : 'mixes into'} ${signalOf(d.bus)?.name || d.bus}: a short press latches, hold to talk`"
        @pointerdown="down(d, $event)"
        @pointerup="up(d)"
        @pointercancel="up(d)"
        @keydown.space.prevent="!$event.repeat && down(d, { currentTarget: null })"
        @keyup.space.prevent="up(d)"
      >
        {{ d.name }}
      </button>
      <button v-if="destinations.length > 1" class="btn talk" :class="{ on: allTalk }" :disabled="!ready" :aria-pressed="allTalk" title="talk to every destination (latched); again to stop" @click="all">All</button>
    </div>
    <div v-if="!compact && destinations.some((d) => d.return)" class="tbbuttons">
      <span class="caption">Listen back</span>
      <button v-for="d in destinations.filter((x) => x.return)" :key="d.id" class="btn small" :class="listening(d) ? 'on-warn' : 'secondary'" :aria-pressed="listening(d)" :title="`the monitors play ${sourceName(d.return)}`" @click="toggleListen(d)">
        {{ d.name }}
      </button>
    </div>
    <p v-if="!destinations.length" class="note">No talkback destinations{{ compact ? "." : ": add one below (a bus: studio, IFB, remote, aux)." }}</p>

    <details v-if="!compact" class="tbsettings">
      <summary>Mics and destinations</summary>
      <table>
        <thead><tr><th>Mic</th><th>Input · channel</th><th>Priority</th><th>Level dB</th><th>Dims the control room</th><th></th></tr></thead>
        <tbody>
          <tr v-for="m in t.mics" :key="m.id" :data-mic="m.id">
            <td><input :value="m.name" aria-label="Mic name" @change="setMic(m.id, { name: $event.target.value })" /></td>
            <td>
              <input type="number" min="1" :value="m.input" aria-label="Input" style="width: 4rem" @change="setMic(m.id, { input: Number($event.target.value) })" />
              <input type="number" min="1" :value="m.channel" aria-label="Channel" style="width: 4rem" @change="setMic(m.id, { channel: Number($event.target.value) })" />
            </td>
            <td><input type="number" min="0" max="9" :value="m.priority" aria-label="Priority" style="width: 4rem" @change="setMic(m.id, { priority: Number($event.target.value) })" /></td>
            <td><input type="number" min="-40" max="20" step="0.5" :value="m.level_db" aria-label="Level" style="width: 5rem" @change="setMic(m.id, { level_db: Number($event.target.value) })" /></td>
            <td><input type="checkbox" :checked="m.dims_control_room" aria-label="Dims the control room" @change="setMic(m.id, { dims_control_room: $event.target.checked })" /></td>
            <td><ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/talkback/mics/${m.id}`))" /></td>
          </tr>
          <tr>
            <td colspan="2" class="muted small">Browser microphones: priority</td>
            <td><input type="number" min="0" max="9" :value="t.browser_priority" aria-label="Browser priority" style="width: 4rem" @change="act(() => api.patch('/api/v1/talkback', { browser_priority: Number($event.target.value) }))" /></td>
            <td colspan="3"></td>
          </tr>
        </tbody>
      </table>
      <button class="btn small secondary" @click="addMic">+ Mic</button>
      <table style="margin-top: 0.6rem">
        <thead><tr><th>Destination</th><th>Bus</th><th>Mode</th><th>Programme while talking</th><th>Level dB</th><th>Return</th><th></th></tr></thead>
        <tbody>
          <tr v-for="d in t.destinations" :key="d.id" :data-destination="d.id">
            <td><input :value="d.name" aria-label="Destination name" @change="setDest(d.id, { name: $event.target.value })" /></td>
            <td>
              <select :value="d.bus" aria-label="Bus" @change="setDest(d.id, { bus: $event.target.value })">
                <option v-for="b in buses" :key="b.id" :value="b.id" :disabled="b.id !== d.bus && t.destinations.some((x) => x.bus === b.id)">{{ b.name }}</option>
              </select>
            </td>
            <td><Segmented :model-value="d.mode" :options="[{ value: 'mix', label: 'Mix' }, { value: 'interrupt', label: 'Interrupt' }]" label="Mode" @update:model-value="(v) => setDest(d.id, { mode: v })" /></td>
            <td><Pot v-if="d.mode === 'interrupt'" label="" unit="dB (−60: cut)" :value="interruptValue(d)" :min="-60" :max="0" :def="-60" :fine="1" @change="(v) => setDest(d.id, { interrupt_db: v <= -60 ? '-inf' : v })" /></td>
            <td><input type="number" min="-40" max="20" step="0.5" :value="d.level_db" aria-label="Level" style="width: 5rem" @change="setDest(d.id, { level_db: Number($event.target.value) })" /></td>
            <td>
              <select :value="returnValue(d)" aria-label="Return" @change="setReturn(d, $event.target.value)">
                <option value="">—</option>
                <option v-for="c in returnChoices" :key="c.value" :value="c.value">{{ c.label }}</option>
              </select>
            </td>
            <td><ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/talkback/destinations/${d.id}`))" /></td>
          </tr>
        </tbody>
      </table>
      <button class="btn small secondary" :disabled="!freeBuses.length" @click="addDest">+ Destination</button>
    </details>
  </div>
</template>
