<script setup>
// The console (§12.1): strips in banks or in a layout's pages, the buses and the VCAs beside them, the
// mute groups in the toolbar. A strip's name or chain opens its processing; a VCA's spill shows only its
// members.
import { computed, ref, watch } from "vue";
import GroupsPanel from "./GroupsPanel.vue";
import Segmented from "./Segmented.vue";
import Strip from "./Strip.vue";
import VcaStrip from "./VcaStrip.vue";
import { SCALES } from "../dsp.js";
import { api } from "../api.js";
import { act, buses, live, muteGroups, patchMuteGroup, spill, store, stored, strips, vcaOf, vcaSignals, vcas } from "../store.js";

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
const byId = (id) => strips.value.find((s) => s.id === id) || buses.value.find((b) => b.id === id);
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

// New strip or bus.
const adding = ref("");
const form = ref({ id: "", name: "", format: "mono" });
function startAdd(kind) {
  adding.value = kind;
  form.value = { id: "", name: "", format: kind === "bus" ? "stereo" : "mono" };
}
async function add() {
  const body = { format: form.value.format };
  if (form.value.id.trim()) body.id = form.value.id.trim();
  if (form.value.name.trim()) body.name = form.value.name.trim();
  const ok = await act(() => api.post(`/api/v1/${adding.value === "bus" ? "buses" : "strips"}`, body), `${adding.value === "bus" ? "Bus" : "Strip"} added.`);
  if (ok) adding.value = "";
}
const FORMATS = ["mono", "stereo", "3.0", "5.1", "7.1", "5.1.4", "7.1.4"];
const editingGroups = ref(false);
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
    </div>
    <div class="actions">
      <button class="btn secondary" @click="adding = ''">Cancel</button>
      <button class="btn" @click="add">Add</button>
    </div>
  </div>

  <div v-if="!live.mix" class="empty">Loading the mix…</div>
  <div v-else-if="!strips.length && !buses.length" class="empty">No strips or buses yet. Add one above, or import a show in Settings.</div>
  <div v-else class="console">
    <div class="strips">
      <Strip v-for="s in shown" :key="s.id" :signal="s" :bus="isBusId(s.id)" :scale="scale" :alignment="alignment" @open="emit('open', $event)" />
      <div v-if="!shown.length" class="empty">This page is empty.</div>
    </div>
    <div v-if="shownBuses.length && !spilled" class="busesec">
      <Strip v-for="b in shownBuses" :key="b.id" :signal="b" bus :scale="scale" :alignment="alignment" @open="emit('open', $event)" />
    </div>
    <div v-if="vcas.length" class="vcasec">
      <VcaStrip v-for="v in vcas" :key="v.id" :vca="v" />
    </div>
  </div>
</template>
