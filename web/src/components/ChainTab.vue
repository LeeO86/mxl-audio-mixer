<script setup>
// The processing chain editor (§12.2): pick a strip or bus; drag a node type from the palette into the
// chain (or tap it to append), drag nodes to reorder them (or use ↑ ↓), bypass, delete (press twice),
// and edit each node. The chain's response is drawn on top.
import { computed, ref } from "vue";
import ChainCurve from "./ChainCurve.vue";
import SignalSettings from "./SignalSettings.vue";
import ConfirmButton from "./ConfirmButton.vue";
import NodeEditor from "./NodeEditor.vue";
import FormatSelect from "./FormatSelect.vue";
import { NODE_TYPES, DYNAMICS, unsupported } from "../nodes.js";
import { api } from "../api.js";
import { act, flush, isBus, live, patchNode, select, signalOf, signalPath, signals } from "../store.js";

const sig = computed(() => signalOf(live.selected) || signals.value[0] || null);
const chain = computed(() => sig.value?.chain || []);
const open = ref({});
const isOpen = (id) => open.value[id] !== false;
const meters = computed(() => live.meters[sig.value?.id]?.nodes || {});
const groups = computed(() => {
  const out = {};
  for (const [type, t] of Object.entries(NODE_TYPES)) (out[t.group] ||= []).push({ type, label: t.label, help: t.help || "" });
  return out;
});

async function add(type, position) {
  if (!sig.value) return;
  const others = signals.value.filter((s) => s.id !== sig.value.id).map((s) => s.id);
  const body = NODE_TYPES[type].make(others);
  if (position !== undefined) body.position = position;
  await act(() => api.post(`${signalPath(sig.value.id)}/chain`, body));
}
async function move(node, position) {
  flush();
  await act(() => api.post(`${signalPath(sig.value.id)}/chain/${encodeURIComponent(node)}/move`, { position }));
}
async function remove(node) {
  await act(() => api.del(`${signalPath(sig.value.id)}/chain/${encodeURIComponent(node)}`));
}
const bypass = (n) => patchNode(sig.value.id, n.id, { bypass: !n.bypass });

// Drag and drop: palette chips carry "type:<t>", node cards "node:<id>".
const dragged = ref("");
const over = ref(-1);
function dragStart(ev, what) {
  dragged.value = what;
  ev.dataTransfer.setData("text/plain", what);
  ev.dataTransfer.effectAllowed = "copyMove";
}
function dragOver(ev, i) {
  ev.preventDefault();
  over.value = i;
}
function drop(ev, i) {
  ev.preventDefault();
  const what = ev.dataTransfer.getData("text/plain") || dragged.value;
  over.value = -1;
  dragged.value = "";
  if (what.startsWith("type:")) add(what.slice(5), i);
  else if (what.startsWith("node:")) {
    const id = what.slice(5);
    const from = chain.value.findIndex((n) => n.id === id);
    if (from < 0) return;
    const to = i > from ? i - 1 : i;
    if (to !== from) move(id, to);
  }
}
const chainCurve = computed(() => chain.value.filter((n) => !n.bypass && ["eq", "filter", "dynamic-eq"].includes(n.type)));
</script>

<template>
  <div v-if="!signals.length" class="empty">No strips or buses yet.</div>
  <div v-else class="chainlayout">
    <div>
      <div class="panel">
        <h3>Signal</h3>
        <select :value="sig?.id" aria-label="Signal" @change="select($event.target.value)">
          <optgroup label="Strips">
            <option v-for="s in live.mix.strips" :key="s.id" :value="s.id">{{ s.name }}</option>
          </optgroup>
          <optgroup label="Buses">
            <option v-for="b in live.mix.buses" :key="b.id" :value="b.id">{{ b.name }}</option>
          </optgroup>
        </select>
        <p class="note">{{ chain.length }} of 32 nodes · latency {{ (live.latency[sig?.id] || 0).toFixed(2) }} ms</p>
        <FormatSelect v-if="sig" :signal="sig" />
      </div>
      <div class="panel palette">
        <h3>Add</h3>
        <p class="note" style="margin-top: 0">Drag into the chain, or tap to append.</p>
        <template v-for="(items, g) in groups" :key="g">
          <div class="group-caption">{{ g }}</div>
          <button v-for="t in items" :key="t.type" class="pchip" draggable="true" :title="t.help" :disabled="chain.length >= 32 || (t.type === 'auto' && chain.some((n) => n.type === 'auto'))" @dragstart="dragStart($event, `type:${t.type}`)" @click="add(t.type)">
            {{ t.label }} <span class="muted">+</span>
          </button>
        </template>
      </div>
    </div>

    <div>
      <SignalSettings v-if="sig" :signal="sig" style="margin-bottom: 0.9rem" />
      <div v-if="chainCurve.length" class="panel" style="margin-bottom: 0.9rem">
        <h3>{{ sig.name }}: chain response</h3>
        <ChainCurve :chain="chainCurve" />
      </div>
      <div class="nodes">
        <div v-for="(n, i) in chain" :key="n.id" class="nodecard" :class="{ bypassed: n.bypass, over: over === i, dragging: dragged === `node:${n.id}` }" @dragover="dragOver($event, i)" @dragleave="over = -1" @drop="drop($event, i)">
          <div class="nhead" draggable="true" @dragstart="dragStart($event, `node:${n.id}`)" @dragend="(dragged = ''), (over = -1)">
            <span class="ntitle">{{ NODE_TYPES[n.type]?.label || n.type }}</span>
            <code>{{ n.id }}</code>
            <span v-if="unsupported(n, sig.format, !isBus(sig.id))" class="state-tag bypass" :title="unsupported(n, sig.format, !isBus(sig.id))">⚠ not on {{ sig.format }}</span>
            <span v-if="DYNAMICS.has(n.type)" class="grbar"><span class="gr"><span :style="{ width: `${Math.min(100, ((meters[n.id]?.gain_reduction_db || 0) / 20) * 100)}%` }"></span></span></span>
            <span class="spacer"></span>
            <button class="btn small bypass" :class="{ secondary: !n.bypass }" :aria-pressed="n.bypass" @click="bypass(n)">Bypass</button>
            <button class="btn small secondary" :disabled="i === 0" :aria-label="`Move ${n.id} up`" @click="move(n.id, i - 1)">↑</button>
            <button class="btn small secondary" :disabled="i === chain.length - 1" :aria-label="`Move ${n.id} down`" @click="move(n.id, i + 1)">↓</button>
            <button class="btn small secondary" :aria-label="isOpen(n.id) ? `Collapse ${n.id}` : `Expand ${n.id}`" @click="open[n.id] = !isOpen(n.id)">{{ isOpen(n.id) ? "▾" : "▸" }}</button>
            <ConfirmButton label="Delete" small @confirm="remove(n.id)" />
          </div>
          <div v-if="isOpen(n.id)" class="nbody">
            <NodeEditor :signal="sig.id" :node="n" />
          </div>
        </div>
        <div class="dropzone" :class="{ over: over === chain.length }" @dragover="dragOver($event, chain.length)" @dragleave="over = -1" @drop="drop($event, chain.length)">
          {{ chain.length ? "Drop here to append" : "No processing yet: drag a node type here, or tap one in the palette." }}
        </div>
      </div>
    </div>
  </div>
</template>
