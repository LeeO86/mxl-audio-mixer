<script setup>
// The control room (§10.1, §10.2): who is soloed and the solo settings, the monitor paths, talkback, the
// source buttons, and the oscillator for line-up.
import { computed, ref } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import MonitorCard from "./MonitorCard.vue";
import PersonalCard from "./PersonalCard.vue";
import TalkbackPanel from "./TalkbackPanel.vue";
import Pot from "./Pot.vue";
import Segmented from "./Segmented.vue";
import { api, dbValue } from "../api.js";
import { act, buses, clearSolo, live, room, signalOf, signals, soloed, toggleSolo } from "../store.js";

const setRoom = (body) => act(() => api.patch("/api/v1/control-room", body));
const solo = computed(() => room.value.solo || {});
const soloList = computed(() => Object.entries(soloed.value).filter(([, k]) => k));
const programme = computed(() => {
  const v = dbValue(solo.value.programme_db);
  return Number.isFinite(v) ? Math.max(-80, v) : -80;
});

// New monitor.
const adding = ref(false);
const form = ref({ id: "", name: "", kind: "speakers", format: "stereo" });
async function addMonitor() {
  const body = { kind: form.value.kind, format: form.value.format };
  if (form.value.id.trim()) body.id = form.value.id.trim();
  if (form.value.name.trim()) body.name = form.value.name.trim();
  if (buses.value[0]) body.source = { signal: buses.value[0].id };
  if (await act(() => api.post("/api/v1/monitors", body), "Monitor added.")) adding.value = false;
}

// Source buttons.
const sourceName = (s) => ("signal" in s ? signalOf(s.signal)?.name || s.signal : `Input ${s.input}`);
const choices = computed(() => [
  ...signals.value.map((x) => ({ value: `s:${x.id}`, label: x.name })),
  ...(live.status?.inputs || []).map((i) => ({ value: `i:${i.input}`, label: `Input ${i.input}` })),
]);
function addSource(value) {
  const s = value.startsWith("s:") ? { signal: value.slice(2) } : { input: Number(value.slice(2)) };
  setRoom({ sources: [...room.value.sources, s] });
}
const removeSource = (i) => setRoom({ sources: room.value.sources.filter((_, k) => k !== i) });

// The oscillator: a bus with an oscillator node (send it anywhere, monitor it, publish it).
const oscillator = computed(() => {
  for (const b of buses.value) {
    const node = b.chain.find((n) => n.type === "oscillator");
    if (node) return { bus: b, node };
  }
  return null;
});
const PRESETS = [
  { label: "1 kHz −18 dBFS (EBU R 68)", body: { waveform: "sine", freq_hz: 1000, level_dbfs: -18 } },
  { label: "1 kHz −20 dBFS (SMPTE RP 155)", body: { waveform: "sine", freq_hz: 1000, level_dbfs: -20 } },
  { label: "EBU stereo ident (R 49)", body: { waveform: "ebu-ident", freq_hz: 1000, level_dbfs: -18 } },
];
async function addOscillator() {
  const ok = await act(() => api.post("/api/v1/buses", { id: "osc", name: "Oscillator", format: "stereo" }));
  if (ok) await act(() => api.post("/api/v1/buses/osc/chain", { id: "tone", type: "oscillator", ...PRESETS[0].body }), "Oscillator added: send it from the routing matrix, or pick it as a monitor source.");
}
const setOsc = (body) => act(() => api.patch(`/api/v1/buses/${oscillator.value.bus.id}/chain/${oscillator.value.node.id}`, body));
</script>

<template>
  <div class="panel">
    <h3>
      Solo
      <span class="spacer"></span>
      <button class="btn small" :disabled="!soloList.length" @click="clearSolo">Clear solos</button>
    </h3>
    <div class="soloed">
      <button v-for="[id, k] in soloList" :key="id" class="chip" :title="`end the ${k.toUpperCase()} of ${signalOf(id)?.name || id}`" @click="toggleSolo(id, k)">{{ k.toUpperCase() }} {{ signalOf(id)?.name || id }} ✕</button>
      <span v-if="!soloList.length" class="muted small">Nothing soloed. PFL and AFL are on the strips.</span>
    </div>
    <div class="params" style="margin-top: 0.5rem">
      <div>
        <label>Mode</label>
        <Segmented :model-value="solo.mode" :options="[{ value: 'additive', label: 'Additive' }, { value: 'interlock', label: 'Interlock' }]" label="Solo mode" @update:model-value="(v) => setRoom({ solo: { mode: v } })" />
      </div>
      <Pot label="PFL trim" unit="dB" :value="solo.pfl_trim_db ?? 0" :min="-24" :max="24" :def="0" :fine="0.5" @change="(v) => setRoom({ solo: { pfl_trim_db: v } })" />
      <Pot label="Programme under solo" unit="dB (−80: −∞)" :value="programme" :min="-80" :max="0" :def="-21" :fine="1" @change="(v) => setRoom({ solo: { programme_db: v <= -80 ? '-inf' : v } })" />
      <div>
        <label class="check" style="margin-top: 1.2rem" title="while anything is soloed, every other strip that is not solo-safe is muted in the mix">
          <input type="checkbox" :checked="solo.sip" @change="setRoom({ solo: { sip: $event.target.checked } })" /> Solo in place (destructive)
        </label>
        <p v-if="solo.sip" class="note">Solo-safe strips stay in the mix: Processing → the strip's settings.</p>
      </div>
    </div>
  </div>

  <div class="toolbar" style="margin-top: 0.9rem">
    <span class="caption">Monitors</span>
    <span class="spacer"></span>
    <button class="btn secondary" @click="adding = !adding">+ Monitor</button>
  </div>
  <div v-if="adding" class="panel addform">
    <div class="row">
      <div><label>Id (empty: automatic)</label><input v-model="form.id" placeholder="automatic" /></div>
      <div><label>Name</label><input v-model="form.name" placeholder="the id" /></div>
      <div>
        <label>Kind</label>
        <select v-model="form.kind"><option value="speakers">Speakers</option><option value="headphones">Headphones</option></select>
      </div>
      <div>
        <label>Format</label>
        <select v-model="form.format"><option v-for="f in ['stereo', 'mono', '3.0', '5.1', '7.1', '5.1.4', '7.1.4']" :key="f" :value="f">{{ f }}</option></select>
      </div>
    </div>
    <div class="actions"><button class="btn secondary" @click="adding = false">Cancel</button><button class="btn" @click="addMonitor">Add</button></div>
  </div>
  <div class="monitors">
    <MonitorCard v-for="m in room.monitors" :key="m.id" :monitor="m" />
    <PersonalCard />
  </div>
  <div v-if="!room.monitors.length" class="empty">No monitors yet: add the speakers and headphones of the control room.</div>

  <TalkbackPanel style="margin-top: 0.9rem" />

  <div class="grid two" style="margin-top: 0.9rem">
    <div class="panel">
      <h3>Source buttons</h3>
      <p class="note" style="margin-top: 0">The buttons every monitor shows (without any: the buses).</p>
      <div class="soloed">
        <span v-for="(s, i) in room.sources" :key="i" class="pill neutral" style="font-size: 0.8rem">{{ sourceName(s) }} <button class="btn small secondary" :aria-label="`Remove ${sourceName(s)}`" @click="removeSource(i)">✕</button></span>
      </div>
      <select :value="''" aria-label="Add a source button" style="margin-top: 0.5rem" @change="addSource($event.target.value), ($event.target.value = '')">
        <option value="" disabled>+ Add a source button</option>
        <option v-for="c in choices" :key="c.value" :value="c.value">{{ c.label }}</option>
      </select>
    </div>
    <div class="panel">
      <h3>Oscillator</h3>
      <template v-if="oscillator">
        <p class="note" style="margin-top: 0">Bus <strong>{{ oscillator.bus.name }}</strong>: send it from the routing matrix, pick it as a monitor source, or publish it.</p>
        <div class="mbuttons">
          <button v-for="p in PRESETS" :key="p.label" class="btn secondary" @click="setOsc(p.body)">{{ p.label }}</button>
        </div>
        <div class="mbuttons">
          <button class="btn on-warn" :class="oscillator.node.bypass ? 'secondary' : ''" :aria-pressed="!oscillator.node.bypass" @click="setOsc({ bypass: !oscillator.node.bypass })">{{ oscillator.node.bypass ? "Off" : "On" }}</button>
          <span class="muted small" style="align-self: center">{{ ({ "ebu-ident": "EBU ident", "second-pulse": "second pulse" })[oscillator.node.waveform] || "sine" }} {{ oscillator.node.freq_hz }} Hz, {{ oscillator.node.level_dbfs }} dBFS</span>
        </div>
        <Pot label="Frequency" unit="Hz" :value="oscillator.node.freq_hz" :min="20" :max="20000" :def="1000" :fine="0.02" :digits="0" log @change="(v) => setOsc({ freq_hz: v })" />
        <Pot label="Level" unit="dBFS" :value="oscillator.node.level_dbfs" :min="-60" :max="0" :def="-18" :fine="0.5" @change="(v) => setOsc({ level_dbfs: v })" />
        <ConfirmButton label="Remove the oscillator bus" small @confirm="act(() => api.del(`/api/v1/buses/${oscillator.bus.id}`))" />
      </template>
      <template v-else>
        <p class="note" style="margin-top: 0">No oscillator yet. It is a bus with an oscillator node: tone presets and the EBU stereo ident.</p>
        <button class="btn" @click="addOscillator">Add an oscillator</button>
      </template>
    </div>
  </div>
</template>
