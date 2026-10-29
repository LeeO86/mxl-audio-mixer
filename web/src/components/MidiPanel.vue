<script setup>
// MIDI control surfaces (A7): connect the browser's MIDI, give each input port a mode (Mackie Control, its
// extender, or generic with MIDI learn) and the output for its feedback, move the Mackie bank, choose the
// master fader's bus, and keep the learnt mappings (export, import, remove). Everything is remembered in
// this browser only.
import { computed, ref } from "vue";
import Modal from "./Modal.vue";
import { download } from "../api.js";
import { exportMappings, importMappings } from "../midicodec.js";
import { controlText, deviceOf, midi, moveBank, removeMapping, setDevice, setMappings, startLearn, startMidi, stopMidi, surface, targetText } from "../midi.js";
import { buses, live } from "../store.js";

const emit = defineEmits(["close"]);

const MODES = [
  ["off", "Off"],
  ["mcu", "Mackie Control"],
  ["mcu-xt", "Mackie Control extender"],
  ["generic", "Generic (MIDI learn)"],
];
const anyMcu = computed(() => midi.inputs.some((i) => deviceOf(i.name).mode === "mcu"));
const anyGeneric = computed(() => midi.inputs.some((i) => deviceOf(i.name).mode === "generic"));
const bankText = computed(() => {
  const list = surface();
  if (!list.length) return "no strips";
  const end = Math.min(list.length, midi.bank + 8);
  return `${midi.bank + 1}–${end} of ${list.length}: ${list.slice(midi.bank, end).map((s) => s.name).join(", ")}`;
});
const origin = typeof location === "undefined" ? "" : location.origin;

function learn() {
  startLearn();
  emit("close");
}
const fileEl = ref(null);
const importError = ref("");
async function importFile(ev) {
  const file = ev.target.files?.[0];
  ev.target.value = "";
  if (!file) return;
  try {
    const list = importMappings(await file.text());
    setMappings(list);
    importError.value = "";
    live.notice = `${list.length} MIDI mapping${list.length === 1 ? "" : "s"} imported.`;
  } catch (e) {
    importError.value = `Import failed: ${e.message}.`;
  }
}
</script>

<template>
  <Modal title="MIDI control" wide @close="emit('close')">
    <div class="midipanel">
      <p v-if="midi.state === 'insecure'" class="warnbox" data-midi-state="insecure">
        Web MIDI needs a secure page: https, or localhost. This page is {{ origin }}. On the platform the mixer is served over HTTPS (behind its proxy), where MIDI works; in a frame, the frame needs
        <code>allow="midi"</code>.
      </p>
      <p v-else-if="midi.state === 'unsupported'" class="warnbox" data-midi-state="unsupported">
        This browser has no Web MIDI. Chrome and Edge have it; Firefox asks for a site permission first (it installs a small add-on), Safari has none.
      </p>
      <p v-else-if="midi.state === 'denied'" class="warnbox" data-midi-state="denied">MIDI access was refused. Allow MIDI for this site in the browser's site settings, then connect again.</p>

      <div class="row tight">
        <button v-if="midi.state !== 'ready'" class="btn" :disabled="midi.state === 'connecting'" @click="startMidi">Connect MIDI</button>
        <button v-else class="btn secondary" @click="stopMidi">Disconnect</button>
        <span v-if="midi.state === 'ready'" class="muted small">{{ midi.inputs.length }} input{{ midi.inputs.length === 1 ? "" : "s" }}, {{ midi.outputs.length }} output{{ midi.outputs.length === 1 ? "" : "s" }}<template v-if="!midi.sysex">; no SysEx permission: the Mackie display stays empty</template></span>
      </div>

      <table v-if="midi.state === 'ready' && midi.inputs.length" class="mididevices">
        <thead>
          <tr><th>Input</th><th>Mode</th><th>Feedback to</th></tr>
        </thead>
        <tbody>
          <tr v-for="i in midi.inputs" :key="i.name" :data-midi-input="i.name">
            <td>{{ i.name }}<span v-if="i.state !== 'connected'" class="muted small"> ({{ i.state }})</span></td>
            <td>
              <select :value="deviceOf(i.name).mode" :aria-label="`${i.name} mode`" @change="setDevice(i.name, { mode: $event.target.value })">
                <option v-for="[v, t] in MODES" :key="v" :value="v">{{ t }}</option>
              </select>
            </td>
            <td>
              <select :value="deviceOf(i.name).output" :aria-label="`${i.name} feedback output`" @change="setDevice(i.name, { output: $event.target.value })">
                <option value="">none</option>
                <option v-for="o in midi.outputs" :key="o.name" :value="o.name">{{ o.name }}</option>
              </select>
            </td>
          </tr>
        </tbody>
      </table>
      <p v-else-if="midi.state === 'ready'" class="empty">No MIDI input. Plug in a device; it shows here at once.</p>

      <template v-if="anyMcu">
        <h4>Mackie Control</h4>
        <p class="note">8 strips per bank over the console's strips, then its buses: fader (motor), mute, solo (PFL), select, the V-pot pans (push: centre). Bank and channel buttons move the bank.</p>
        <div class="row tight">
          <button class="btn small secondary" aria-label="Bank left" @click="moveBank(-8)">«</button>
          <button class="btn small secondary" aria-label="Channel left" @click="moveBank(-1)">‹</button>
          <span class="small" data-midi-bank>{{ bankText }}</span>
          <button class="btn small secondary" aria-label="Channel right" @click="moveBank(1)">›</button>
          <button class="btn small secondary" aria-label="Bank right" @click="moveBank(8)">»</button>
        </div>
        <div class="row tight">
          <label class="small">Master fader</label>
          <select v-model="midi.master" aria-label="Master fader bus" style="width: auto">
            <option value="">the first bus</option>
            <option v-for="b in buses" :key="b.id" :value="b.id">{{ b.name }}</option>
          </select>
        </div>
      </template>

      <h4>MIDI learn</h4>
      <p class="note">
        Learn mode: click a fader, mute, PFL, AFL or pan on the console, then move a control on a device in generic mode. A button toggles on its press; a fader, knob or pitch bend sets the
        value. Kept in this browser; export them to take them to another one.
      </p>
      <div class="row tight">
        <button class="btn" :disabled="midi.state !== 'ready' || !anyGeneric" :title="anyGeneric ? '' : 'set a device to Generic first'" @click="learn">Learn…</button>
        <button class="btn secondary" :disabled="!midi.mappings.length" @click="download('mixer-midi.json', exportMappings(midi.mappings))">Export</button>
        <button class="btn secondary" @click="fileEl.click()">Import…</button>
        <input ref="fileEl" type="file" accept=".json,application/json" hidden aria-label="MIDI mappings file" @change="importFile" />
        <button class="btn secondary" :disabled="!midi.mappings.length" @click="setMappings([])">Remove all</button>
      </div>
      <p v-if="importError" class="warnbox">{{ importError }}</p>
      <table v-if="midi.mappings.length" class="midimaps">
        <tbody>
          <tr v-for="(m, i) in midi.mappings" :key="i" :data-midi-map="i">
            <td>{{ controlText(m.control) }}</td>
            <td class="muted small">{{ m.device }}</td>
            <td>→ {{ targetText(m.target) }}</td>
            <td style="text-align: right"><button class="btn small secondary" :aria-label="`Remove mapping ${i + 1}`" @click="removeMapping(i)">Remove</button></td>
          </tr>
        </tbody>
      </table>
      <p v-else class="muted small">No mappings yet.</p>
    </div>
  </Modal>
</template>
