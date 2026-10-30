<script setup>
// The console (§12.1): strips in banks or in a layout's pages, the buses and the VCAs beside them, the
// mute groups in the toolbar. A strip's gear or chain opens its processing; a VCA's spill shows only its
// members. A divider between the strips and the buses sets how wide each side is (C2); each scrolls on
// its own.
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import AutomationPanel from "./AutomationPanel.vue";
import AutomixPanel from "./AutomixPanel.vue";
import GroupsPanel from "./GroupsPanel.vue";
import MidiPanel from "./MidiPanel.vue";
import OperationsPanel from "./OperationsPanel.vue";
import RemoteDesk from "./RemoteDesk.vue";
import Segmented from "./Segmented.vue";
import Strip from "./Strip.vue";
import VcaStrip from "./VcaStrip.vue";
import { SCALES } from "../dsp.js";
import { api } from "../api.js";
import { numbered } from "../console.js";
import { midi, stopLearn, targetText } from "../midi.js";
import { act, buses, live, muteGroups, patchMuteGroup, select, spill, store, stored, strips, vcaOf, vcaSignals, vcas } from "../store.js";

const emit = defineEmits(["open"]);

const view = ref(stored("console.view", "all")); // "all" or a layout name
const page = ref(Number(stored("console.page", 0)) || 0);
const bankSize = ref(Number(stored("console.bank", 16)) || 16);
const scale = ref(stored("meter.scale", "dbfs"));
const alignment = ref(Number(stored("meter.alignment", -18)));
watch(view, (v) => (store("console.view", v), (page.value = 0)));
watch(page, (v) => store("console.page", v));
watch(bankSize, (v) => (store("console.bank", v), (page.value = 0)));
watch(scale, (v) => store("meter.scale", v));

const layouts = computed(() => live.mix?.layouts || []);
const layout = computed(() => layouts.value.find((l) => l.name === view.value) || null);
const viewOptions = computed(() => [{ value: "all", label: "All strips" }, ...layouts.value.map((l) => ({ value: l.name, label: l.name }))]);
const banks = computed(() => Math.max(1, Math.ceil(strips.value.length / bankSize.value)));
const pageOptions = computed(() =>
  layout.value
    ? layout.value.pages.map((p, i) => ({ value: i, label: p.name }))
    : Array.from({ length: banks.value }, (_, i) => ({ value: i, label: `${i * bankSize.value + 1}–${Math.min(strips.value.length, (i + 1) * bankSize.value)}` })),
);
const byId = (id) => strips.value.find((s) => s.id === id) || buses.value.find((b) => b.id === id) || vcaOf(id);
const isVcaId = (id) => !!vcaOf(id);
const spilled = computed(() => (spill.vca ? vcaOf(spill.vca) : null));
const shown = computed(() => {
  if (spilled.value) {
    const ids = new Set(vcaSignals(spilled.value.id));
    return [...strips.value, ...buses.value].filter((x) => ids.has(x.id));
  }
  if (layout.value) {
    const p = layout.value.pages[Math.min(page.value, layout.value.pages.length - 1)];
    return (p?.signals || []).map(byId).filter(Boolean);
  }
  return strips.value.slice(page.value * bankSize.value, (page.value + 1) * bankSize.value);
});
const isBusId = (id) => buses.value.some((b) => b.id === id);
const shownBuses = computed(() => buses.value.filter((b) => !shown.value.includes(b)));

// New strips or buses: several at once get numbered names and ids (C6); the first new one is selected
// and shown (C4).
const adding = ref("");
const form = ref({ id: "", name: "", format: "mono", count: 1 });
function startAdd(kind) {
  adding.value = kind;
  form.value = { id: "", name: "", format: kind === "bus" ? "stereo" : "mono", count: 1 };
}
const MAX_ADD = 64;
async function add() {
  const kind = adding.value;
  const count = Math.max(1, Math.min(MAX_ADD, Math.round(Number(form.value.count) || 1)));
  const id = form.value.id.trim();
  const name = form.value.name.trim();
  const names = numbered(name, count);
  const made = [];
  for (let i = 0; i < count; i++) {
    const body = { format: form.value.format };
    if (id) body.id = count > 1 ? `${id}-${i + 1}` : id;
    if (name) body.name = names[i];
    const r = await act(() => api.post(`/api/v1/${kind === "bus" ? "buses" : "strips"}`, body));
    if (!r) break;
    made.push(r.id);
  }
  if (!made.length) return;
  const what = kind === "bus" ? "Bus" : "Strip";
  live.notice = made.length === 1 ? `${what} added.` : `${made.length} ${what === "Bus" ? "buses" : "strips"} added${made.length < count ? ` (of ${count})` : ""}.`;
  if (made.length === count) adding.value = "";
  select(made[0]);
  reveal(made[0]);
}
/** Shows a signal: the bank with a new strip (in "All strips"), then scrolls it into view. */
async function reveal(id) {
  if (kind(id) === "strip" && !layout.value && !spilled.value) {
    const at = strips.value.findIndex((s) => s.id === id);
    if (at >= 0) page.value = Math.floor(at / bankSize.value);
  }
  // The mix with the new signal comes on the WebSocket: wait for it to be drawn.
  for (let i = 0; i < 20; i++) {
    await nextTick();
    const el = document.querySelector(`[data-signal="${CSS.escape(id)}"]`);
    if (el) return el.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
    await new Promise((r) => setTimeout(r, 100));
  }
}
const kind = (id) => (strips.value.some((s) => s.id === id) ? "strip" : "bus");

// The divider (C2): the width of the buses' side in px, remembered in this browser; 0 is "as wide as they
// need, up to half".
const busWidth = ref(Number(stored("console.buswidth", 0)) || 0);
watch(busWidth, (v) => store("console.buswidth", Math.round(v)));
const consoleEl = ref(null);
const mastersEl = ref(null);
let drag = null;
function dragStart(e) {
  drag = { x: e.clientX, w: mastersEl.value?.getBoundingClientRect().width || 0 };
  window.addEventListener("pointermove", dragMove);
  window.addEventListener("pointerup", dragEnd);
  e.preventDefault();
}
const limit = (w) => Math.max(120, Math.min((consoleEl.value?.getBoundingClientRect().width || 1600) - 160, w));
function dragMove(e) {
  if (drag) busWidth.value = limit(drag.w - (e.clientX - drag.x));
}
function dragEnd() {
  drag = null;
  window.removeEventListener("pointermove", dragMove);
  window.removeEventListener("pointerup", dragEnd);
}
function dragKey(e) {
  const step = e.shiftKey ? 120 : 24;
  const w = busWidth.value || mastersEl.value?.getBoundingClientRect().width || 300;
  if (e.key === "ArrowLeft") busWidth.value = limit(w + step);
  else if (e.key === "ArrowRight") busWidth.value = limit(w - step);
  else if (e.key === "Home" || e.key === "Escape") busWidth.value = 0;
  else return;
  e.preventDefault();
}
onUnmounted(dragEnd);
const FORMATS = ["mono", "stereo", "3.0", "5.1", "7.1", "5.1.4", "7.1.4"];
const editingGroups = ref(false);
const operating = ref(false);
const automating = ref(false);
const automixing = ref(false);
const midiOpen = ref(false);
</script>

<template>
  <div class="toolbar">
    <div class="group">
      <span class="caption">View</span>
      <Segmented v-model="view" :options="viewOptions" label="Console view" />
    </div>
    <div class="group">
      <span class="caption">{{ layout ? "Page" : "Bank" }}</span>
      <Segmented v-model="page" :options="pageOptions" label="Page" />
      <select v-if="!layout" v-model.number="bankSize" aria-label="Strips per bank" style="width: auto">
        <option v-for="n in [8, 12, 16, 24, 32, 64]" :key="n" :value="n">{{ n }} per bank</option>
      </select>
    </div>
    <span class="spacer"></span>
    <div class="group">
      <span class="caption">Meters</span>
      <select v-model="scale" aria-label="Meter scale" style="width: auto">
        <option v-for="(s, k) in SCALES" :key="k" :value="k">{{ s.label }}</option>
      </select>
    </div>
    <div class="group">
      <button class="btn secondary" @click="startAdd('strip')">+ Strip</button>
      <button class="btn secondary" @click="startAdd('bus')">+ Bus</button>
      <button class="btn secondary" :aria-pressed="editingGroups" @click="editingGroups = !editingGroups">VCAs &amp; groups</button>
      <button class="btn secondary" :aria-pressed="operating" title="group, N-1, link, mix-minus" @click="operating = !operating">Operations</button>
      <button class="btn secondary" :aria-pressed="automixing" title="gain-sharing auto-mixes" @click="automixing = !automixing">Auto-mix</button>
      <button class="btn secondary" :aria-pressed="automating" title="auto-level, ducking, voice detection" @click="automating = !automating">Auto</button>
      <button class="btn secondary" :aria-pressed="midiOpen" :class="{ on: midi.state === 'ready' }" title="MIDI control surfaces: Mackie Control, MIDI learn" @click="midiOpen = !midiOpen">MIDI</button>
    </div>
  </div>
  <div v-if="muteGroups.length" class="toolbar mutegroups">
    <span class="caption">Mute groups</span>
    <button
      v-for="g in muteGroups"
      :key="g.id"
      class="btn on-bad"
      :class="g.active ? '' : 'secondary'"
      :aria-pressed="g.active"
      :title="`${g.members.length} members; mute-safe members stay`"
      @click="patchMuteGroup(g.id, { active: !g.active })"
    >
      {{ g.name }}
    </button>
  </div>
  <GroupsPanel v-if="editingGroups" @close="editingGroups = false" />
  <OperationsPanel v-if="operating" @close="operating = false" />
  <AutomationPanel v-if="automating" @close="automating = false" />
  <AutomixPanel v-if="automixing" @close="automixing = false" />
  <MidiPanel v-if="midiOpen" @close="midiOpen = false" />
  <div v-if="midi.learning" class="banner info midilearn" data-midi-learn>
    <span>
      MIDI learn:
      <template v-if="midi.target"><strong>{{ targetText(midi.target) }}</strong> picked; now move a control on the MIDI device.</template>
      <template v-else>click a fader, mute, PFL, AFL or pan, then move a control on the MIDI device.</template>
      <template v-if="midi.learnt"> Learnt: {{ midi.learnt }}.</template>
    </span>
    <span class="spacer"></span>
    <button class="btn small" @click="stopLearn">Done</button>
  </div>
  <div v-if="spilled" class="banner info spillbanner">
    Spill: VCA <strong>{{ spilled.name }}</strong>, {{ shown.length }} members
    <button class="btn small secondary" @click="spill.vca = null">Show all</button>
  </div>

  <div v-if="adding" class="panel addform">
    <h3>New {{ adding }}</h3>
    <div class="row">
      <div><label>Id (a-z, 0-9, -; empty: automatic)</label><input v-model="form.id" placeholder="automatic" /></div>
      <div><label>Name</label><input v-model="form.name" placeholder="the id" /></div>
      <div>
        <label>Format</label>
        <select v-model="form.format"><option v-for="f in FORMATS" :key="f" :value="f">{{ f }}</option></select>
      </div>
      <div class="countfield">
        <label>How many</label><input v-model.number="form.count" type="number" min="1" :max="MAX_ADD" step="1" :aria-label="`Number of new ${adding === 'bus' ? 'buses' : 'strips'}`" />
      </div>
    </div>
    <p v-if="form.count > 1" class="note">
      {{ form.name.trim() ? `Named ${numbered(form.name.trim(), 2).join(", ")} …` : "Named after their ids" }}<template v-if="form.id.trim()">; ids {{ form.id.trim() }}-1, {{ form.id.trim() }}-2 …</template>
    </p>
    <div class="actions">
      <button class="btn secondary" @click="adding = ''">Cancel</button>
      <button class="btn" @click="add">Add</button>
    </div>
  </div>

  <div v-if="!live.mix" class="empty">Loading the mix…</div>
  <div v-else-if="!strips.length && !buses.length" class="empty">No strips or buses yet. Add one above, or import a show in Settings.</div>
  <div v-else ref="consoleEl" class="console">
    <div class="strips">
      <template v-for="s in shown" :key="s.id">
        <VcaStrip v-if="isVcaId(s.id)" :vca="s" />
        <Strip v-else :signal="s" :bus="isBusId(s.id)" :scale="scale" :alignment="alignment" @open="emit('open', $event)" />
      </template>
      <div v-if="!shown.length" class="empty">This page is empty.</div>
    </div>
    <template v-if="(shownBuses.length && !spilled) || vcas.length">
      <div
        class="cdivider"
        role="separator"
        aria-orientation="vertical"
        aria-label="Divider between strips and buses"
        tabindex="0"
        title="drag to share the width; arrow keys too, Home: back to automatic"
        @pointerdown="dragStart"
        @dblclick="busWidth = 0"
        @keydown="dragKey"
      ></div>
      <div ref="mastersEl" class="masters" :style="busWidth ? { flex: `0 0 ${busWidth}px`, width: `${busWidth}px` } : null">
        <div v-if="shownBuses.length && !spilled" class="busesec">
          <Strip v-for="b in shownBuses" :key="b.id" :signal="b" bus :scale="scale" :alignment="alignment" @open="emit('open', $event)" />
        </div>
        <div v-if="vcas.length" class="vcasec">
          <VcaStrip v-for="v in vcas" :key="v.id" :vca="v" />
        </div>
      </div>
    </template>
  </div>
  <RemoteDesk :scale="scale" :alignment="alignment" />
</template>
