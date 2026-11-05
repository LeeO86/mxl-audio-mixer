<script setup>
// The console (§12.1): strips in banks or in a view's banks, the buses and the VCAs beside them, the
// mute groups in the toolbar. A strip's gear or chain opens its processing; a VCA's spill shows only its
// members. A divider between the strips and the buses sets how wide each side is (C2); each scrolls on
// its own. 1.6.0: the view and bank choice sit next to the tabs (B4); besides "All strips" a view of only
// the input strips, one of only the buses, and the show's custom views (B3, ViewEditor); strips and
// buses are dragged by their grip into another place (B2: the show's order; in a custom view, the
// view's); a click selects strips, Shift a range, Ctrl one more (B1); the strip and bus menus hold what
// the toolbar had (B5–B9), as dialogs (B10).
import { computed, nextTick, onUnmounted, ref, watch, watchEffect } from "vue";
import AutomationPanel from "./AutomationPanel.vue";
import AutomixPanel from "./AutomixPanel.vue";
import GroupsPanel from "./GroupsPanel.vue";
import MidiPanel from "./MidiPanel.vue";
import Modal from "./Modal.vue";
import OperationsPanel from "./OperationsPanel.vue";
import RemoteDesk from "./RemoteDesk.vue";
import Segmented from "./Segmented.vue";
import Strip from "./Strip.vue";
import StripMenu from "./StripMenu.vue";
import VcaStrip from "./VcaStrip.vue";
import ViewEditor from "./ViewEditor.vue";
import { api } from "../api.js";
import { numbered } from "../console.js";
import { consoleUi } from "../consoleui.js";
import { midi, stopLearn, targetText } from "../midi.js";
import { act, buses, clearSelection, live, muteGroups, patchMuteGroup, select, selection, spill, store, stored, strips, vcaOf, vcaSignals, vcas } from "../store.js";

const emit = defineEmits(["open"]);

// The view (per browser): "all", ":inputs", ":buses" or a custom view's name (the show's layouts).
const view = ref(stored("console.view", "all"));
const page = ref(Number(stored("console.page", 0)) || 0);
const bankSize = ref(Number(stored("console.bank", 16)) || 16);
const alignment = ref(Number(stored("meter.alignment", -18)));
watch(view, (v) => (store("console.view", v), (page.value = 0)));
watch(page, (v) => store("console.page", v));
watch(bankSize, (v) => (store("console.bank", v), (page.value = 0)));
const scale = computed(() => consoleUi.scale);

const layouts = computed(() => live.mix?.layouts || []);
const layout = computed(() => layouts.value.find((l) => l.name === view.value) || null);
const onlyBuses = computed(() => view.value === ":buses");
const onlyInputs = computed(() => view.value === ":inputs");
const viewOptions = computed(() => [
  { value: "all", label: "All strips" },
  { value: ":inputs", label: "Inputs", title: "only the input strips" },
  { value: ":buses", label: "Buses", title: "only the buses" },
  ...layouts.value.map((l) => ({ value: l.name, label: l.name, title: "a custom view (the show's)" })),
]);
// A view that is gone (deleted by another operator) falls back to all strips.
watch(layouts, () => {
  if (!["all", ":inputs", ":buses"].includes(view.value) && !layout.value && live.mix) view.value = "all";
});
const banked = computed(() => (onlyBuses.value ? buses.value : strips.value));
const banks = computed(() => Math.max(1, Math.ceil(banked.value.length / bankSize.value)));
const pageOptions = computed(() =>
  layout.value
    ? layout.value.pages.map((p, i) => ({ value: i, label: p.name }))
    : Array.from({ length: banks.value }, (_, i) => ({ value: i, label: banked.value.length ? `${i * bankSize.value + 1}–${Math.min(banked.value.length, (i + 1) * bankSize.value)}` : "–" })),
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
  return banked.value.slice(page.value * bankSize.value, (page.value + 1) * bankSize.value);
});
const isBusId = (id) => buses.value.some((b) => b.id === id);
// The buses and VCAs beside the strips: in "All strips" only (a custom view places them itself).
const besides = computed(() => view.value === "all");
const shownBuses = computed(() => (besides.value && !spilled.value ? buses.value.filter((b) => !shown.value.includes(b)) : []));
const shownVcas = computed(() => (besides.value || spilled.value ? vcas.value : []));
// The console's order, for a Shift-click range (B1).
watchEffect(() => (selection.order = [...shown.value, ...shownBuses.value].map((x) => x.id)));
const selectedCount = computed(() => selection.ids.filter((id) => byId(id)).length);

// Moving a strip or bus (B2) by its grip, with the mouse or a finger: dropped on another one, it takes
// that one's place. In a custom view the view's bank changes; otherwise the show's order of the strips
// (or buses).
const moving = ref(null); // { id, over }
function gripDown(e, id) {
  moving.value = { id, over: "" };
  e.target.setPointerCapture?.(e.pointerId);
  e.preventDefault();
}
function gripMove(e) {
  if (!moving.value) return;
  const el = document.elementFromPoint(e.clientX, e.clientY)?.closest("[data-slot]");
  moving.value.over = el && el.dataset.slot !== moving.value.id ? el.dataset.slot : "";
}
async function gripUp() {
  const m = moving.value;
  moving.value = null;
  if (m?.over) await moveTo(m.id, m.over);
}
function gripKey(e, id) {
  const list = (isBusId(id) && besides.value && !layout.value ? shownBuses.value : shown.value).map((x) => x.id);
  const i = list.indexOf(id);
  const to = e.key === "ArrowLeft" ? list[i - 1] : e.key === "ArrowRight" ? list[i + 1] : null;
  if (!to) return;
  e.preventDefault();
  moveTo(id, to);
}
async function moveTo(id, over) {
  if (layout.value) {
    const layoutsNew = JSON.parse(JSON.stringify(layouts.value));
    const l = layoutsNew.find((x) => x.name === layout.value.name);
    const p = l.pages[Math.min(page.value, l.pages.length - 1)];
    const from = p.signals.indexOf(id);
    const to = p.signals.indexOf(over);
    if (from < 0 || to < 0) return;
    p.signals.splice(from, 1);
    p.signals.splice(to, 0, id);
    await act(() => api.put("/api/v1/layouts", layoutsNew));
    return;
  }
  const bus = isBusId(id);
  if (bus !== isBusId(over)) return (live.notice = "A strip moves among the strips, a bus among the buses.");
  const list = bus ? buses.value : strips.value;
  const position = list.findIndex((x) => x.id === over);
  if (position >= 0) await act(() => api.post(`/api/v1/${bus ? "buses" : "strips"}/${encodeURIComponent(id)}/move`, { position }));
}
onUnmounted(() => {
  moving.value = null;
  consoleUi.panel = "";
});
const editingViews = ref(false);

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
  if (!layout.value && !spilled.value && (kind(id) === "bus") === onlyBuses.value) {
    const at = banked.value.findIndex((s) => s.id === id);
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
// The dialogs a menu opens (B6–B9): consoleUi.add for a new strip or bus, consoleUi.panel for the rest.
watch(
  () => consoleUi.add,
  (k) => k && startAdd(k),
);
watch(adding, (k) => (consoleUi.add = k));
const panelOpen = (p) => consoleUi.panel === p;
const togglePanel = (p) => (consoleUi.panel = consoleUi.panel === p ? "" : p);
const closePanel = () => (consoleUi.panel = "");
// The menu of an empty side (B6): a placeholder strip with its menu.
const emptyMenu = ref(""); // "strip" or "bus"
</script>

<template>
  <Teleport to="#tabtools" defer>
    <div class="group">
      <span class="caption">View</span>
      <Segmented v-model="view" :options="viewOptions" label="Console view" />
      <button class="btn small secondary" title="custom views: strips, VCAs and buses in banks" data-edit-views @click="editingViews = true">Edit views…</button>
    </div>
    <div class="group">
      <span class="caption">Bank</span>
      <Segmented v-model="page" :options="pageOptions" label="Bank" />
      <select v-if="!layout" v-model.number="bankSize" aria-label="Strips per bank" style="width: auto">
        <option v-for="n in [8, 12, 16, 24, 32, 64]" :key="n" :value="n">{{ n }} per bank</option>
      </select>
    </div>
  </Teleport>
  <div class="toolbar">
    <div v-if="selectedCount > 1" class="group selinfo" data-selection>
      <span><strong>{{ selectedCount }}</strong> selected: linked</span>
      <button class="btn small secondary" title="keep only the strip of the processing view" @click="clearSelection">Clear</button>
    </div>
    <span class="spacer"></span>
    <div class="group">
      <button class="btn secondary" :aria-pressed="panelOpen('groups')" @click="togglePanel('groups')">VCAs &amp; groups</button>
      <button class="btn secondary" :aria-pressed="panelOpen('midi')" :class="{ on: midi.state === 'ready' }" title="MIDI control surfaces: Mackie Control, MIDI learn" @click="togglePanel('midi')">MIDI</button>
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
  <!-- The groups panel stays beside the console: its VCAs are spilled and tried while it is open. -->
  <GroupsPanel v-if="panelOpen('groups')" @close="closePanel" />
  <Modal v-if="panelOpen('operations')" title="Operations" wide @close="closePanel"><OperationsPanel @close="closePanel" /></Modal>
  <Modal v-if="panelOpen('auto')" title="Auto-level, ducking and voice detection" wide @close="closePanel"><AutomationPanel @close="closePanel" /></Modal>
  <AutomixPanel v-if="panelOpen('automix')" @close="closePanel" />
  <MidiPanel v-if="panelOpen('midi')" @close="closePanel" />
  <ViewEditor v-if="editingViews" :view="layout?.name || ''" @close="editingViews = false" @saved="(n) => n && (view = n)" />
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

  <Modal v-if="adding" :title="`New ${adding}`" @close="adding = ''">
    <div class="row addform">
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
  </Modal>
  <StripMenu v-if="emptyMenu" :bus="emptyMenu === 'bus'" @close="emptyMenu = ''" />

  <div v-if="!live.mix" class="empty">Loading the mix…</div>
  <div v-else ref="consoleEl" class="console" :class="{ moving: !!moving }" @pointermove="gripMove" @pointerup="gripUp" @pointercancel="moving = null">
    <div class="strips">
      <div v-if="!(onlyBuses ? buses : strips).length && !layout && !spilled" class="slot">
        <section class="strip placeholder" :class="{ bus: onlyBuses }" :data-placeholder="onlyBuses ? 'bus' : 'strip'">
          <div class="shead">
            <span class="sname muted">{{ onlyBuses ? "No bus" : "No strip" }}</span>
            <button class="btn small secondary" :aria-label="onlyBuses ? 'Bus menu' : 'Strip menu'" @click="emptyMenu = onlyBuses ? 'bus' : 'strip'">Menu…</button>
          </div>
          <p class="muted small">Its menu adds one.</p>
        </section>
      </div>
      <div v-for="s in shown" :key="s.id" class="slot" :class="{ over: moving?.over === s.id, lifted: moving?.id === s.id }" :data-slot="s.id">
        <div
          v-if="!spilled"
          class="grip"
          role="button"
          tabindex="0"
          :aria-label="`Move ${s.name}`"
          title="drag to move (arrow keys too)"
          @pointerdown="gripDown($event, s.id)"
          @keydown="gripKey($event, s.id)"
        ></div>
        <VcaStrip v-if="isVcaId(s.id)" :vca="s" />
        <Strip v-else :signal="s" :bus="isBusId(s.id)" :scale="scale" :alignment="alignment" @open="emit('open', $event)" />
      </div>
      <div v-if="!shown.length && (layout || spilled)" class="empty">This bank is empty.</div>
    </div>
    <template v-if="shownBuses.length || shownVcas.length || (besides && !buses.length && !spilled)">
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
        <div v-if="shownBuses.length" class="busesec">
          <div v-for="b in shownBuses" :key="b.id" class="slot" :class="{ over: moving?.over === b.id, lifted: moving?.id === b.id }" :data-slot="b.id">
            <div class="grip" role="button" tabindex="0" :aria-label="`Move ${b.name}`" title="drag to move (arrow keys too)" @pointerdown="gripDown($event, b.id)" @keydown="gripKey($event, b.id)"></div>
            <Strip :signal="b" bus :scale="scale" :alignment="alignment" @open="emit('open', $event)" />
          </div>
        </div>
        <div v-else-if="besides && !buses.length && !spilled" class="busesec">
          <div class="slot">
            <section class="strip bus placeholder" data-placeholder="bus">
              <div class="shead">
                <span class="sname muted">No bus</span>
                <button class="btn small secondary" aria-label="Bus menu" @click="emptyMenu = 'bus'">Menu…</button>
              </div>
              <p class="muted small">Its menu adds one.</p>
            </section>
          </div>
        </div>
        <div v-if="shownVcas.length" class="vcasec">
          <VcaStrip v-for="v in shownVcas" :key="v.id" :vca="v" />
        </div>
      </div>
    </template>
  </div>
  <RemoteDesk :scale="scale" :alignment="alignment" />
</template>
