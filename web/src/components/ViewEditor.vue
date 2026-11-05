<script setup>
// The console's custom views (1.6.0 B3): named views of banks in which strips, VCAs and buses are placed
// freely. They are the show's layouts (PUT /api/v1/layouts), so every operator has the same views. A click
// on an item of the palette puts it at the end of the bank picked; drag items to move them between banks
// or within one; × takes one out. Saved as a whole.
import { computed, ref } from "vue";
import Modal from "./Modal.vue";
import { api } from "../api.js";
import { act, buses, live, selection, strips, vcas } from "../store.js";

const props = defineProps({ view: { type: String, default: "" } });
const emit = defineEmits(["close", "saved"]);

const draft = ref(JSON.parse(JSON.stringify(live.mix?.layouts || [])));
const at = ref(Math.max(0, draft.value.findIndex((l) => l.name === props.view)));
const current = computed(() => draft.value[at.value] || null);
const bank = ref(0);
const nameOf = (id) => [...strips.value, ...buses.value, ...vcas.value].find((x) => x.id === id)?.name || id;
const kindOf = (id) => (strips.value.some((s) => s.id === id) ? "strip" : vcas.value.some((v) => v.id === id) ? "vca" : "bus");
const palette = computed(() => [
  { label: "Inputs", items: strips.value },
  { label: "VCAs", items: vcas.value },
  { label: "Buses", items: buses.value },
]);
const placed = computed(() => new Set((current.value?.pages || []).flatMap((p) => p.signals)));

function newView() {
  let n = draft.value.length + 1;
  while (draft.value.some((l) => l.name === `View ${n}`)) n++;
  draft.value.push({ name: `View ${n}`, pages: [{ name: "Bank 1", signals: [] }] });
  at.value = draft.value.length - 1;
  bank.value = 0;
}
function removeView() {
  draft.value.splice(at.value, 1);
  at.value = Math.max(0, at.value - 1);
  bank.value = 0;
}
function addBank() {
  const pages = current.value.pages;
  pages.push({ name: `Bank ${pages.length + 1}`, signals: [] });
  bank.value = pages.length - 1;
}
function removeBank(i) {
  current.value.pages.splice(i, 1);
  bank.value = Math.min(bank.value, current.value.pages.length - 1);
}
function put(id) {
  const p = current.value?.pages[bank.value];
  if (p && !p.signals.includes(id)) p.signals.push(id);
}
const addSelected = () => selection.ids.forEach(put);
const take = (p, i) => p.signals.splice(i, 1);

// Drag and drop of the placed items: onto an item (before it) or onto a bank (at its end).
let dragged = null; // { page, index }
function dragStart(e, pi, i) {
  dragged = { page: pi, index: i };
  e.dataTransfer.effectAllowed = "move";
  e.dataTransfer.setData("text/plain", current.value.pages[pi].signals[i]);
}
function drop(pi, i = -1) {
  if (!dragged) return;
  const pages = current.value.pages;
  const [id] = pages[dragged.page].signals.splice(dragged.index, 1);
  let to = i < 0 ? pages[pi].signals.length : i;
  if (dragged.page === pi && dragged.index < to && i >= 0) to--;
  pages[pi].signals.splice(to, 0, id);
  dragged = null;
}

const problem = computed(() => {
  const names = draft.value.map((l) => l.name.trim());
  if (names.some((n) => !n || n.length > 64)) return "Every view needs a name (1 to 64 characters).";
  if (new Set(names).size !== names.length) return "Two views have the same name.";
  if (names.some((n) => ["all", ":inputs", ":buses"].includes(n))) return "That name is a built-in view.";
  if (draft.value.some((l) => !l.pages.length)) return "Every view needs a bank.";
  if (draft.value.some((l) => l.pages.some((p) => !p.name.trim()))) return "Every bank needs a name.";
  return "";
});
async function save() {
  const body = draft.value.map((l) => ({ name: l.name.trim(), pages: l.pages.map((p) => ({ name: p.name.trim(), signals: p.signals })) }));
  if (await act(() => api.put("/api/v1/layouts", body), "Views saved.")) {
    emit("saved", current.value?.name.trim() || "");
    emit("close");
  }
}
</script>

<template>
  <Modal title="Views" wide @close="emit('close')">
    <p class="note" style="margin-top: 0">A view is banks of strips, VCAs and buses in any order. Views are part of the show: every operator sees the same.</p>
    <div class="row tight viewpick">
      <select v-model.number="at" aria-label="View to edit" :disabled="!draft.length">
        <option v-for="(l, i) in draft" :key="i" :value="i">{{ l.name || "(no name)" }}</option>
      </select>
      <input v-if="current" v-model="current.name" maxlength="64" aria-label="View name" />
      <button class="btn secondary" @click="newView">+ View</button>
      <button v-if="current" class="btn secondary danger" @click="removeView">Delete view</button>
    </div>
    <div v-if="!draft.length" class="empty">No custom views yet.</div>
    <template v-if="current">
      <div class="viewbanks">
        <div
          v-for="(p, pi) in current.pages"
          :key="pi"
          class="viewbank"
          :class="{ picked: bank === pi }"
          :data-bank="pi"
          @click="bank = pi"
          @dragover.prevent
          @drop.prevent="drop(pi)"
        >
          <div class="row tight">
            <input v-model="p.name" maxlength="64" :aria-label="`Bank ${pi + 1} name`" />
            <span class="muted small">{{ p.signals.length }}</span>
            <span class="spacer"></span>
            <button class="btn small secondary" :aria-label="`Delete bank ${p.name}`" @click.stop="removeBank(pi)">×</button>
          </div>
          <div class="viewitems">
            <span
              v-for="(id, i) in p.signals"
              :key="id"
              class="viewitem"
              :class="`k-${kindOf(id)}`"
              draggable="true"
              :data-item="id"
              @dragstart="dragStart($event, pi, i)"
              @dragover.prevent
              @drop.prevent.stop="drop(pi, i)"
              >{{ nameOf(id) }}<button class="x" :aria-label="`Take ${nameOf(id)} out`" @click.stop="take(p, i)">×</button></span
            >
            <span v-if="!p.signals.length" class="muted small">empty: pick items below</span>
          </div>
        </div>
        <button class="btn secondary" @click="addBank">+ Bank</button>
      </div>
      <h4>Put into {{ current.pages[bank]?.name || "the bank" }}</h4>
      <div v-for="g in palette" :key="g.label" class="viewpalette">
        <span class="caption">{{ g.label }}</span>
        <button v-for="x in g.items" :key="x.id" class="btn small secondary" :class="{ placed: placed.has(x.id) }" :data-put="x.id" @click="put(x.id)">{{ x.name }}</button>
        <span v-if="!g.items.length" class="muted small">none</span>
      </div>
      <button v-if="selection.ids.length" class="btn small secondary" @click="addSelected">Put the {{ selection.ids.length }} selected</button>
    </template>
    <p v-if="problem" class="warnbox">{{ problem }}</p>
    <div class="actions">
      <button class="btn secondary" @click="emit('close')">Cancel</button>
      <button class="btn" :disabled="!!problem" @click="save">Save views</button>
    </div>
  </Modal>
</template>
