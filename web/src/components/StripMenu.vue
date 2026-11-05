<script setup>
// The menu of a strip or bus (1.6.0 B5–B11), as a dialog over the mixer (B10): its processing, format and
// routing (the strip's own dialogs, `choose`), copy its settings, its auto-mix, MIDI learn, the meter scale,
// auto and operations, a new strip or bus, delete. With several selected, the actions run on all of them
// (B1). Without a signal (an empty console), only what makes sense then: a new strip or bus, the meters.
import { computed, ref } from "vue";
import Modal from "./Modal.vue";
import { SCALES } from "../dsp.js";
import { copyBody } from "../link.js";
import { startLearn } from "../midi.js";
import { consoleUi } from "../consoleui.js";
import { automixOf, automixes, buses, isBus, linkedWith, live, patchAutomix, patchBatch, signalOf, strips } from "../store.js";

const props = defineProps({
  signal: { type: Object, default: null },
  bus: { type: Boolean, default: false },
});
const emit = defineEmits(["close", "choose"]);

const s = computed(() => props.signal);
const kind = computed(() => (props.bus ? "bus" : "strip"));
// The selected signals this menu acts on (it, or every selected one when it is one of them).
const group = computed(() => (s.value ? linkedWith(s.value.id).filter((id) => signalOf(id)) : []));
const others = computed(() => Math.max(0, group.value.length - 1));
const title = computed(() => {
  if (!s.value) return props.bus ? "Buses" : "Strips";
  return others.value ? `${s.value.name} and ${others.value} more selected` : s.value.name;
});
const step = ref("menu"); // "menu", "copy", "automix"

function choose(what) {
  emit("choose", what);
  emit("close");
}
function panel(what) {
  consoleUi.panel = what;
  emit("close");
}
function add() {
  consoleUi.add = kind.value;
  emit("close");
}
function learn() {
  startLearn({ signal: s.value.id, param: "fader" });
  emit("close");
}

// Copy (B11): every setting and the processing, not the label, to one other signal of its kind or to the
// selected ones, in one request.
const copyTargets = computed(() => (props.bus ? buses.value : strips.value).filter((x) => x.id !== s.value?.id));
const selectedTargets = computed(() => group.value.filter((id) => id !== s.value?.id && isBus(id) === props.bus));
function copyTo(ids) {
  const signals = {};
  for (const id of ids) {
    const to = signalOf(id);
    if (to) signals[id] = copyBody(s.value, to);
  }
  if (!Object.keys(signals).length) return;
  patchBatch({ signals });
  live.notice = `Settings of ${s.value.name} copied to ${ids.map((id) => signalOf(id)?.name || id).join(", ")}.`;
  emit("close");
}

// Auto-mix (B7): the strips join one auto-mix (and leave any other) or none.
const amStrips = computed(() => group.value.filter((id) => !isBus(id)));
const current = computed(() => {
  const ams = new Set(amStrips.value.map((id) => automixOf(id)?.id || ""));
  return ams.size === 1 ? [...ams][0] : null; // null: mixed
});
function assign(amId) {
  const ids = new Set(amStrips.value);
  for (const a of automixes.value) {
    const kept = a.members.filter((m) => !ids.has(m.signal));
    const members = a.id === amId ? [...kept, ...[...ids].map((signal) => a.members.find((m) => m.signal === signal) || { signal, weight_db: 0, bypass: false })] : kept;
    if (members.length !== a.members.length || a.id === amId) patchAutomix(a.id, { members });
  }
  emit("close");
}
</script>

<template>
  <Modal :title="title" @close="emit('close')">
    <div v-if="step === 'menu'" class="menulist" role="menu" data-strip-menu>
      <p v-if="others" class="note">The actions run on all {{ group.length }} selected.</p>
      <template v-if="s">
        <button role="menuitem" @click="choose('processing')">Processing…</button>
        <button role="menuitem" @click="choose('format')">Format…</button>
        <button role="menuitem" @click="choose('send-to')">Send to…</button>
        <button v-if="bus" role="menuitem" @click="choose('add-source')">Add source…</button>
        <button v-if="bus" role="menuitem" @click="choose('subgroup')">{{ s.subgroup ? "Plain bus…" : "Subgroup…" }}</button>
        <button role="menuitem" @click="step = 'copy'">Copy settings…</button>
        <button v-if="!bus" role="menuitem" @click="step = 'automix'">Auto-mix…</button>
        <button role="menuitem" :disabled="others > 0" title="pick its fader, then move a control on the MIDI device" @click="learn">MIDI learn</button>
      </template>
      <div class="menurow">
        <span>Meters</span>
        <select v-model="consoleUi.scale" aria-label="Meter scale">
          <option v-for="(m, k) in SCALES" :key="k" :value="k">{{ m.label }}</option>
        </select>
      </div>
      <button role="menuitem" title="auto-level, ducking, voice detection" @click="panel('auto')">Auto…</button>
      <button role="menuitem" title="group, N-1, link, mix-minus" @click="panel('operations')">Operations…</button>
      <button role="menuitem" @click="add">+ {{ bus ? "Bus" : "Strip" }}…</button>
      <button v-if="s" role="menuitem" class="danger" @click="choose('delete')">Delete…</button>
    </div>

    <template v-else-if="step === 'copy'">
      <p class="note">Every setting and the processing of {{ s.name }} (not its name, input, format or sends) go to the {{ kind }} you pick; the {{ kind }}'s own are replaced.</p>
      <div v-if="selectedTargets.length" class="actions" style="justify-content: flex-start">
        <button class="btn" data-copy-selected @click="copyTo(selectedTargets)">To the {{ selectedTargets.length }} selected</button>
      </div>
      <div class="pickgrid">
        <button v-for="x in copyTargets" :key="x.id" class="btn secondary" :data-pick="x.id" @click="copyTo([x.id])">{{ x.name }}</button>
      </div>
      <div v-if="!copyTargets.length" class="empty">No other {{ kind }}.</div>
      <div class="actions"><button class="btn secondary" @click="step = 'menu'">Back</button></div>
    </template>

    <template v-else-if="step === 'automix'">
      <p class="note">A strip is in one gain-sharing auto-mix at most.</p>
      <div class="pickgrid">
        <button class="btn" :class="current === '' ? '' : 'secondary'" data-automix="" @click="assign('')">None</button>
        <button v-for="a in automixes" :key="a.id" class="btn" :class="current === a.id ? '' : 'secondary'" :data-automix="a.id" @click="assign(a.id)">{{ a.name }}</button>
      </div>
      <div class="actions">
        <button class="btn secondary" @click="step = 'menu'">Back</button>
        <button class="btn secondary" @click="panel('automix')">Add auto-mix…</button>
      </div>
    </template>
  </Modal>
</template>
