<script setup>
// Logic graphs (§11): blocks wired together, evaluated at 100 Hz. The canvas (§11.3) takes blocks from
// the palette (drag or click), draws cables between ports, zooms and pans; every edit is one step of
// undo and redo, and selected blocks copy and paste with their wires. The list under it edits the same
// graph. Live values are polled while the tab is open.
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import { VueFlow, useVueFlow } from "@vue-flow/core";
import { MiniMap } from "@vue-flow/minimap";
import { Controls } from "@vue-flow/controls";
import "@vue-flow/core/dist/style.css";
import "@vue-flow/minimap/dist/style.css";
import "@vue-flow/controls/dist/style.css";
import ConfirmButton from "./ConfirmButton.vue";
import LogicNode from "./LogicNode.vue";
import { api, fmtDb } from "../api.js";
import { act, live, signals } from "../store.js";
import { clipboard, copyOf, forget, freshId, history, historyOf, pasted, record, sameGraph } from "../logicEdit.js";

const { applyNodeChanges, applyEdgeChanges, fitView, screenToFlowCoordinate, getSelectedNodes, getSelectedEdges } = useVueFlow("logic");

const graphs = computed(() => live.mix?.logic || []);
const selected = ref("");
const graph = computed(() => graphs.value.find((g) => g.id === selected.value) || graphs.value[0] || null);
const name = ref("");
// Updated in place, so that only what shows a value redraws on each poll: redrawing the whole tab would
// reset a number being typed in the list.
const values = reactive({});
const buttons = ref([]);
let timer = 0;

// [type, label, palette group]
const TYPES = [
  ["const", "Number", "in"],
  ["button", "Button", "in"],
  ["mute", "Mute state", "in"],
  ["fader", "Fader", "in"],
  ["vad", "Voice", "in"],
  ["level", "Level", "in"],
  ["above", "Above", "in"],
  ["tally", "On air", "in"],
  ["schedule", "Schedule", "in"],
  ["and", "AND", "logic"],
  ["or", "OR", "logic"],
  ["xor", "XOR", "logic"],
  ["not", "NOT", "logic"],
  ["compare", "Compare", "logic"],
  ["edge", "Rising edge", "logic"],
  ["latch", "Latch", "logic"],
  ["timer", "Timer", "logic"],
  ["delay", "Delay", "logic"],
  ["debounce", "Debounce", "logic"],
  ["set-mute", "Set mute", "act"],
  ["set-fader", "Set fader", "act"],
  ["recall", "Recall", "act"],
  ["webhook", "Webhook", "act"],
];
const GROUPS = [
  ["in", "Inputs"],
  ["logic", "Logic"],
  ["act", "Actions"],
];
const typeLabel = (type) => TYPES.find((t) => t[0] === type)?.[1] || type;
const groupOf = (type) => TYPES.find((t) => t[0] === type)?.[2] || "logic";
const withSignal = ["mute", "fader", "vad", "level", "above", "set-mute", "set-fader"];
const MS_TYPES = ["timer", "delay", "debounce"];
const sinks = ["set-mute", "set-fader", "recall", "webhook"];
const inputsOf = {
  and: ["a", "b"],
  or: ["a", "b"],
  xor: ["a", "b"],
  compare: ["a", "b"],
  not: ["in"],
  edge: ["in"],
  latch: ["set", "reset"],
  timer: ["in"],
  delay: ["in"],
  debounce: ["in"],
  recall: ["in"],
  webhook: ["in"],
  "set-mute": ["in"],
  "set-fader": ["level"],
};
// The ranges the server accepts (src/mix/logic.rs).
const minute = (n) => Number.isInteger(n) && n >= 0 && n <= 1439;
const CHECK = {
  value: (n) => Number.isFinite(n),
  index: (n) => Number.isInteger(n) && n >= 0 && n <= 65535,
  ms: (n) => Number.isFinite(n) && n >= 0 && n <= 86400000,
  threshold_db: (n) => Number.isFinite(n) && n >= -120 && n <= 20,
  hysteresis_db: (n) => Number.isFinite(n) && n >= 0 && n <= 60,
  start_minute: minute,
  end_minute: minute,
};

const addType = ref("const");
const addSignal = ref("");
const addIndex = ref("");
const addMs = ref("");
const addThreshold = ref("");
const addHysteresis = ref("");
const addSnapshot = ref("");
const addUrl = ref("");
const addStart = ref("");
const addEnd = ref("");
const snapshots = computed(() => live.mix?.snapshots || []);
const wire = ref({ from: "", to: "", port: "in" });

// ---- edits ------------------------------------------------------------------------------------
// One change at a time, each read from the server first: two quick edits would otherwise each send the
// graph they saw, and the slower one would erase the other. A change of blocks or wires is recorded for
// undo with the graph before it and the graph the server answered.
let queued = Promise.resolve();
// A failed step must not stop the ones after it.
const failed = (e) => (live.actionError = e.message);
function commit(label, mutate, id = graph.value?.id) {
  if (!id) return queued;
  queued = queued.then(async () => {
    const current = await api.get(`/api/v1/logic/${id}`).catch(() => null);
    if (!current) return;
    const body = mutate(current);
    if (!body) return;
    const done = await act(() => api.patch(`/api/v1/logic/${id}`, body));
    if (done && done !== true && !sameGraph(current, done)) record(id, label, current, done);
  }).catch(failed);
  return queued;
}

function patch(body) {
  return commit("", () => body);
}

// Undo (back) or redo, only while the server still has what this client last wrote.
function step(back) {
  const id = graph.value?.id;
  if (!id) return queued;
  queued = queued.then(async () => {
    const h = historyOf(id);
    const from = back ? h.undo : h.redo;
    const entry = from.at(-1);
    if (!entry) return;
    const current = await api.get(`/api/v1/logic/${id}`).catch(() => null);
    if (!current) return;
    if (!sameGraph(current, back ? entry.after : entry.before)) {
      forget(id);
      live.notice = "The graph was changed elsewhere: its undo history is cleared.";
      return;
    }
    const done = await act(() => api.patch(`/api/v1/logic/${id}`, back ? entry.before : entry.after), `${back ? "Undone" : "Redone"}: ${entry.label}.`);
    if (!done) return;
    from.pop();
    (back ? h.redo : h.undo).push(entry);
  }).catch(failed);
  return queued;
}
const undo = () => step(true);
const redo = () => step(false);
const undoNext = computed(() => history[graph.value?.id]?.undo.at(-1)?.label || "");
const redoNext = computed(() => history[graph.value?.id]?.redo.at(-1)?.label || "");

async function addGraph() {
  const body = {};
  if (name.value.trim()) body.name = name.value.trim();
  const made = await act(() => api.post("/api/v1/logic", body), "Logic graph added.");
  if (made) {
    name.value = "";
    selected.value = made.id;
  }
}

async function removeGraph(id) {
  if (await act(() => api.del(`/api/v1/logic/${id}`))) forget(id);
}

// The parameters a new block starts with: the fields next to "+ Block" when filled, else a default. A
// string is why the block cannot be made.
function paramsFor(type) {
  const p = {};
  const num = (field, raw, fallback) => {
    if (String(raw).trim() === "") return fallback;
    const n = Number(raw);
    return CHECK[field](n) ? n : undefined;
  };
  if (withSignal.includes(type)) {
    p.signal = addSignal.value || signals.value[0]?.id;
    if (!p.signal) return "This block needs a strip or bus, and there is none.";
  }
  if (type === "const") p.value = 0;
  if (type === "compare") p.op = "gt";
  if (type === "tally" && (p.index = num("index", addIndex.value, 1)) === undefined) return "The on-air index is 0 to 65535.";
  if (MS_TYPES.includes(type) && (p.ms = num("ms", addMs.value, 1000)) === undefined) return "The time is 0 to 86400000 ms.";
  if (type === "schedule") {
    p.start_minute = num("start_minute", addStart.value, 480);
    p.end_minute = num("end_minute", addEnd.value, 1080);
    if (p.start_minute === undefined || p.end_minute === undefined) return "Schedule minutes are 0 to 1439.";
  }
  if (type === "above") {
    p.threshold_db = num("threshold_db", addThreshold.value, -40);
    p.hysteresis_db = num("hysteresis_db", addHysteresis.value, 6);
    if (p.threshold_db === undefined || p.hysteresis_db === undefined) return "The threshold is −120 to 20 dB, the hysteresis 0 to 60 dB.";
  }
  if (type === "recall") {
    p.snapshot = addSnapshot.value || snapshots.value[0]?.id;
    if (!p.snapshot) return "A recall needs a snapshot, and there is none.";
  }
  if (type === "webhook") {
    if (!/^https?:\/\//.test(addUrl.value)) return "Enter the webhook's http(s) URL next to + Block first.";
    p.url = addUrl.value;
  }
  return p;
}

// A new block at `at` (canvas coordinates), or in a row when not given; it is selected.
function place(type, at) {
  const params = paramsFor(type);
  if (typeof params === "string") {
    live.actionError = params;
    return;
  }
  commit(`Add ${typeLabel(type)}`, (g) => {
    const id = freshId(type, new Set(g.blocks.map((b) => b.id)));
    const pos = at || { x: 40 + g.blocks.length * 220, y: 80 };
    want = new Set([id]);
    return { blocks: [...g.blocks, { id, type, x: Math.round(pos.x), y: Math.round(pos.y), ...params }] };
  });
}

function addBlock() {
  place(addType.value);
}

function removeBlock(id) {
  commit(`Delete ${id}`, (g) => ({
    blocks: g.blocks.filter((b) => b.id !== id),
    wires: g.wires.filter((w) => w.from_block !== id && w.to_block !== id),
  }));
}

function setField(b, field, raw) {
  const value = field in CHECK ? Number(raw) : raw;
  if (field in CHECK ? !CHECK[field](value) : !value) return;
  if (field === "url" && !/^https?:\/\//.test(value)) return;
  commit(`Set ${b.id}`, (g) => ({ blocks: g.blocks.map((x) => (x.id === b.id ? { ...x, [field]: value } : x)) }));
}

const wireKey = (w) => `${w.from_block}.${w.from_port}>${w.to_block}.${w.to_port}`;

function addWire() {
  const { from, to, port } = wire.value;
  if (!from || !to || !port) return;
  connect(from, to, port);
}

function connect(from, to, port) {
  commit("Connect", (g) => {
    const wires = g.wires.filter((w) => !(w.to_block === to && w.to_port === port));
    return { wires: [...wires, { from_block: from, from_port: "out", to_block: to, to_port: port }] };
  });
}

function removeWire(w) {
  commit("Delete cable", (g) => ({ wires: g.wires.filter((x) => wireKey(x) !== wireKey(w)) }));
}

const portsOf = (id) => inputsOf[graph.value?.blocks.find((b) => b.id === id)?.type] || [];

// ---- selection, delete, copy, paste ------------------------------------------------------------

// Delete on the canvas reports blocks and cables one by one; they are removed as one edit.
let doomed = null;
function doom(kind, id) {
  if (!doomed) {
    doomed = { blocks: new Set(), wires: new Set() };
    setTimeout(removeDoomed);
  }
  doomed[kind].add(id);
}
function removeDoomed() {
  const { blocks, wires } = doomed;
  doomed = null;
  commit(blocks.size ? `Delete ${blocks.size} block${blocks.size > 1 ? "s" : ""}` : "Delete cable", (g) => {
    const next = {
      blocks: g.blocks.filter((b) => !blocks.has(b.id)),
      wires: g.wires.filter((w) => !blocks.has(w.from_block) && !blocks.has(w.to_block) && !wires.has(wireKey(w))),
    };
    return next.blocks.length === g.blocks.length && next.wires.length === g.wires.length ? null : next;
  });
}

const selectedIds = computed(() => getSelectedNodes.value.map((n) => n.id));
function deleteSelection() {
  for (const id of selectedIds.value) doom("blocks", id);
  for (const e of getSelectedEdges.value) doom("wires", e.id);
}

function copy() {
  const g = graph.value;
  const ids = selectedIds.value;
  if (!g || !ids.length) return false;
  Object.assign(clipboard, copyOf(g, ids), { pastes: 0 });
  live.notice = `${ids.length} block${ids.length > 1 ? "s" : ""} copied.`;
  return true;
}

// The clipboard into the open graph (any graph), with new ids, moved down and right a bit more on each paste.
function paste() {
  if (!graph.value || !clipboard.blocks.length) return false;
  const clip = { blocks: clipboard.blocks, wires: clipboard.wires };
  const offset = 40 * ++clipboard.pastes;
  commit(`Paste ${clip.blocks.length} block${clip.blocks.length > 1 ? "s" : ""}`, (g) => {
    const p = pasted(g, clip, offset);
    want = new Set(p.ids);
    return { blocks: [...g.blocks, ...p.blocks], wires: [...g.wires, ...p.wires] };
  });
  return true;
}

// Ctrl/Cmd + Z, Shift+Z (or Y), C, V while no text field has the focus.
function onKey(e) {
  if (!graph.value || !(e.ctrlKey || e.metaKey) || e.altKey) return;
  if (e.target?.closest?.("input, textarea, select, [contenteditable='true']")) return;
  const k = e.key.toLowerCase();
  let done = true;
  if (k === "z" && !e.shiftKey) undo();
  else if ((k === "z" && e.shiftKey) || k === "y") redo();
  else if (k === "c") done = copy();
  else if (k === "v") done = paste();
  else done = false;
  if (done) e.preventDefault();
}

// ---- palette ----------------------------------------------------------------------------------

const DRAG = "application/x-mixer-logic-block";
const canvas = ref(null);
function onPaletteDrag(e, type) {
  e.dataTransfer.setData(DRAG, type);
  e.dataTransfer.effectAllowed = "copy";
}
function onDragOver(e) {
  if (!e.dataTransfer?.types.includes(DRAG)) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = "copy";
}
function onDrop(e) {
  const type = e.dataTransfer?.getData(DRAG);
  if (!type) return;
  e.preventDefault();
  const at = screenToFlowCoordinate({ x: e.clientX, y: e.clientY });
  place(type, { x: at.x - 70, y: at.y - 20 });
}
// A click (or a touch, where dragging does not work) puts the block in the middle of the view.
function addFromPalette(type) {
  const r = canvas.value?.getBoundingClientRect();
  if (!r) return place(type);
  const n = (graph.value?.blocks.length || 0) % 6;
  const at = screenToFlowCoordinate({ x: r.left + r.width / 2 + n * 16, y: r.top + r.height / 2 + n * 16 });
  place(type, { x: at.x - 70, y: at.y - 20 });
}

// ---- live values ------------------------------------------------------------------------------

async function refresh() {
  const g = graph.value;
  if (!g) return;
  const d = await api.get(`/api/v1/logic/${g.id}`).catch(() => null);
  if (d) {
    const v = d.values || {};
    for (const k of Object.keys(values)) if (!(k in v)) delete values[k];
    Object.assign(values, v);
    showValues();
    buttons.value = d.buttons || [];
  }
}

function press(block, pressed) {
  const g = graph.value;
  return act(() => api.put(`/api/v1/logic/${g.id}/buttons/${block.id}`, { pressed }));
}

const DB = new Set(["fader", "level", "set-fader"]);
const NUMERIC = new Set(["const", ...DB]);
function shown(b) {
  const v = values[b.id];
  if (v === undefined || v === null) return "";
  if (DB.has(b.type)) return `${fmtDb(v)} dB`;
  return NUMERIC.has(b.type) ? String(Number(Number(v).toFixed(2))) : v >= 0.5 ? "on" : "off";
}
function isOn(b) {
  const v = values[b.id];
  return NUMERIC.has(b.type) || v === undefined || v === null ? null : v >= 0.5;
}

function labelOf(b) {
  const type = typeLabel(b.type);
  if (b.type === "tally") return `${type}: ${b.index}`;
  if (b.type === "timer") return `${type}: ${b.ms} ms`;
  if (b.type === "recall") return `${type}: ${snapshots.value.find((s) => s.id === b.snapshot)?.name || b.snapshot}`;
  if (b.type === "delay" || b.type === "debounce") return `${type}: ${b.ms} ms`;
  if (b.type === "schedule") return `${type}: ${b.start_minute}–${b.end_minute}`;
  if (b.type === "webhook") return type;
  const who = b.signal ? signals.value.find((s) => s.id === b.signal)?.name || b.signal : "";
  return who ? `${type}: ${who}` : type;
}

// ---- canvas -----------------------------------------------------------------------------------

const nodes = ref([]);
const edges = ref([]);
// Fit shows the whole graph, but a few blocks are not blown up past 125 %.
const FIT = { padding: 0.2, maxZoom: 1.25 };
const fit = () => fitView(FIT);
let want = null; // block ids to select once they are drawn (a paste, a new block)

function nodeData(b) {
  return { id: b.id, label: labelOf(b), note: b.id, kind: groupOf(b.type), inputs: inputsOf[b.type] || [], output: !sinks.includes(b.type), live: shown(b), on: isOn(b) };
}

function rebuild() {
  const g = graph.value;
  if (!g) {
    nodes.value = [];
    edges.value = [];
    return;
  }
  let keep = new Set(nodes.value.filter((n) => n.selected).map((n) => n.id));
  if (want && g.blocks.some((b) => want.has(b.id))) {
    keep = want;
    want = null;
  }
  nodes.value = g.blocks.map((b) => ({
    id: b.id,
    type: "logic",
    position: { x: b.x || 0, y: b.y || 0 },
    selected: keep.has(b.id),
    data: nodeData(b),
  }));
  const drawn = g.wires.map((w) => ({
    id: wireKey(w),
    source: w.from_block,
    target: w.to_block,
    sourceHandle: "out",
    targetHandle: w.to_port,
    label: w.to_port,
    data: w,
  }));
  nextTick(() => (edges.value = drawn));
}

watch(() => (graph.value ? JSON.stringify({ b: graph.value.blocks, w: graph.value.wires }) : ""), rebuild, { immediate: true });
function showValues() {
  for (const n of nodes.value) {
    const b = graph.value?.blocks.find((x) => x.id === n.id);
    if (b) n.data = nodeData(b);
  }
}
// The list's value cell: a component of its own, so that a new value redraws only it.
const LiveText = { props: { block: { type: Object, required: true } }, setup: (p) => () => shown(p.block) };
// Another graph: start with all of it in view.
watch(
  () => graph.value?.id,
  () => setTimeout(fit, 50),
);

function onNodesChange(changes) {
  applyNodeChanges(changes.filter((c) => c.type !== "remove"));
  for (const c of changes) if (c.type === "remove") doom("blocks", c.id);
}
function onEdgesChange(changes) {
  applyEdgeChanges(changes.filter((c) => c.type !== "remove"));
  for (const c of changes) if (c.type === "remove") doom("wires", c.id);
}
function onDragStop({ node, nodes: dragged }) {
  const moved = Object.fromEntries((dragged?.length ? dragged : [node]).map((n) => [n.id, { x: Math.round(n.position.x), y: Math.round(n.position.y) }]));
  const count = Object.keys(moved).length;
  commit(count > 1 ? `Move ${count} blocks` : "Move block", (g) => {
    let any = false;
    const blocks = g.blocks.map((b) => {
      const m = moved[b.id];
      if (!m || (m.x === b.x && m.y === b.y)) return b;
      any = true;
      return { ...b, ...m };
    });
    return any ? { blocks } : null;
  });
}
function isValidConnection(c) {
  if (c.id) return true;
  const ports = inputsOf[graph.value?.blocks.find((b) => b.id === c.target)?.type] || [];
  return c.source !== c.target && ports.includes(c.targetHandle) && (c.sourceHandle === "out" || !c.sourceHandle);
}
function onConnect(c) {
  if (!isValidConnection(c) || c.id) return;
  connect(c.source, c.target, c.targetHandle);
}

onMounted(() => {
  refresh();
  timer = setInterval(refresh, 250);
  window.addEventListener("keydown", onKey);
});
onUnmounted(() => {
  clearInterval(timer);
  window.removeEventListener("keydown", onKey);
});
</script>

<template>
  <div class="panel">
    <h3>Logic</h3>
    <p class="muted">Blocks run 100 times a second. A loop is allowed when an edge, a latch, a timer, a delay or a debounce is in it, because those remember. Actions run when their input changes. A timer stays on for its duration after its input rises. Above stays on until the level falls by its hysteresis. Recall runs a snapshot when its input rises.</p>
    <div class="row">
      <select :value="graph?.id || ''" aria-label="Logic graph" @change="selected = $event.target.value">
        <option value="">graph…</option>
        <option v-for="g in graphs" :key="g.id" :value="g.id">{{ g.name }}</option>
      </select>
      <input v-model="name" placeholder="name" aria-label="New logic graph name" />
      <button class="btn small" @click="addGraph">+ Graph</button>
    </div>

    <template v-if="graph">
      <div class="row" style="margin-top: 0.6rem">
        <input :value="graph.name" aria-label="Logic name" @change="patch({ name: $event.target.value })" />
        <label class="check"><input type="checkbox" :checked="graph.enabled" @change="patch({ enabled: $event.target.checked })" /> on</label>
        <label class="check" title="values are computed, actions do not run"><input type="checkbox" :checked="graph.dry_run" @change="patch({ dry_run: $event.target.checked })" /> dry run</label>
        <ConfirmButton label="Remove" small @confirm="removeGraph(graph.id)" />
      </div>

      <div class="row tight ltools">
        <button class="btn small secondary" :disabled="!undoNext" :title="undoNext ? `Undo: ${undoNext} (Ctrl+Z)` : 'Nothing to undo'" aria-label="Undo" @click="undo">Undo</button>
        <button class="btn small secondary" :disabled="!redoNext" :title="redoNext ? `Redo: ${redoNext} (Ctrl+Shift+Z)` : 'Nothing to redo'" aria-label="Redo" @click="redo">Redo</button>
        <button class="btn small secondary" :disabled="!selectedIds.length" title="Copy the selected blocks and the cables between them (Ctrl+C)" aria-label="Copy blocks" @click="copy">Copy</button>
        <button class="btn small secondary" :disabled="!clipboard.blocks.length" title="Paste into this graph (Ctrl+V)" aria-label="Paste blocks" @click="paste">Paste</button>
        <button class="btn small secondary" :disabled="!selectedIds.length && !getSelectedEdges.length" title="Delete the selected blocks and cables (Delete)" aria-label="Delete selection" @click="deleteSelection">Delete</button>
        <button class="btn small secondary" @click="fit">Fit</button>
        <span class="muted small lhint">Shift-drag selects an area; Ctrl-click adds a block to the selection.</span>
      </div>

      <div class="logiceditor">
        <aside class="lpalette" aria-label="Block palette">
          <template v-for="[group, caption] in GROUPS" :key="group">
            <div class="group-caption">{{ caption }}</div>
            <button
              v-for="[type, label] in TYPES.filter((t) => t[2] === group)"
              :key="type"
              class="pitem"
              :class="`k-${group}`"
              draggable="true"
              :data-palette="type"
              :title="`Drag onto the canvas, or click to add ${label}`"
              @dragstart="onPaletteDrag($event, type)"
              @click="addFromPalette(type)"
            >
              {{ label }}
            </button>
          </template>
        </aside>
        <div ref="canvas" class="graphwrap logiccanvas" @dragover="onDragOver" @drop="onDrop">
          <VueFlow
            id="logic"
            v-model:nodes="nodes"
            :edges="edges"
            :apply-default="false"
            :is-valid-connection="isValidConnection"
            :delete-key-code="['Delete', 'Backspace']"
            :multi-selection-key-code="['Meta', 'Control']"
            :min-zoom="0.2"
            :max-zoom="3"
            @pane-ready="fit"
            class="graph"
            @nodes-change="onNodesChange"
            @edges-change="onEdgesChange"
            @node-drag-stop="onDragStop"
            @connect="onConnect"
          >
            <template #node-logic="p"><LogicNode :data="p.data" /></template>
            <MiniMap pannable zoomable :width="160" :height="100" />
            <Controls :show-interactive="false" :fit-view-params="FIT" />
          </VueFlow>
        </div>
      </div>

      <table class="amtable" style="margin-top: 0.6rem">
        <thead>
          <tr><th>Block</th><th>Type</th><th></th><th>Value</th><th></th></tr>
        </thead>
        <tbody>
          <tr v-for="b in graph.blocks" :key="b.id" :data-block="b.id" :class="{ chosen: selectedIds.includes(b.id) }">
            <td>{{ b.id }}</td>
            <td>{{ typeLabel(b.type) }}</td>
            <td class="lparams">
              <select v-if="withSignal.includes(b.type)" :value="b.signal" :aria-label="`${b.id} signal`" @change="setField(b, 'signal', $event.target.value)">
                <option v-for="s in signals" :key="s.id" :value="s.id">{{ s.name }}</option>
              </select>
              <input v-if="b.type === 'const'" type="number" :value="b.value" :aria-label="`${b.id} value`" style="width: 5rem" @change="setField(b, 'value', $event.target.value)" />
              <input v-if="b.type === 'tally'" type="number" min="0" max="65535" :value="b.index" :aria-label="`${b.id} index`" style="width: 5rem" @change="setField(b, 'index', $event.target.value)" />
              <input v-if="MS_TYPES.includes(b.type)" type="number" min="0" max="86400000" :value="b.ms" :aria-label="`${b.id} milliseconds`" style="width: 6rem" @change="setField(b, 'ms', $event.target.value)" />
              <template v-if="b.type === 'above'">
                <input type="number" min="-120" max="20" :value="b.threshold_db" :aria-label="`${b.id} threshold`" style="width: 5rem" @change="setField(b, 'threshold_db', $event.target.value)" />
                <input type="number" min="0" max="60" :value="b.hysteresis_db" :aria-label="`${b.id} hysteresis`" style="width: 4rem" @change="setField(b, 'hysteresis_db', $event.target.value)" />
              </template>
              <template v-if="b.type === 'schedule'">
                <input type="number" min="0" max="1439" :value="b.start_minute" :aria-label="`${b.id} start minute`" style="width: 5rem" @change="setField(b, 'start_minute', $event.target.value)" />
                <input type="number" min="0" max="1439" :value="b.end_minute" :aria-label="`${b.id} end minute`" style="width: 5rem" @change="setField(b, 'end_minute', $event.target.value)" />
              </template>
              <input v-if="b.type === 'webhook'" :value="b.url" :aria-label="`${b.id} URL`" style="width: 14rem" @change="setField(b, 'url', $event.target.value)" />
              <select v-if="b.type === 'compare'" :value="b.op" :aria-label="`${b.id} comparison`" @change="setField(b, 'op', $event.target.value)">
                <option value="gt">a &gt; b</option>
                <option value="lt">a &lt; b</option>
                <option value="eq">a = b</option>
              </select>
              <select v-if="b.type === 'recall'" :value="b.snapshot" :aria-label="`${b.id} snapshot`" @change="setField(b, 'snapshot', $event.target.value)">
                <option v-for="s in snapshots" :key="s.id" :value="s.id">{{ s.name }}</option>
              </select>
              <button v-if="b.type === 'button'" class="btn small" :class="{ primary: buttons.includes(b.id) }" :aria-pressed="buttons.includes(b.id)" @click="press(b, !buttons.includes(b.id))">
                {{ buttons.includes(b.id) ? "Pressed" : "Press" }}
              </button>
            </td>
            <td class="num" :data-value="b.id"><LiveText :block="b" /></td>
            <td><button class="btn small secondary" :aria-label="`Remove ${b.id}`" @click="removeBlock(b.id)">×</button></td>
          </tr>
        </tbody>
      </table>

      <div class="row" style="margin-top: 0.4rem">
        <select v-model="addType" aria-label="Block type">
          <option v-for="[id, label] in TYPES" :key="id" :value="id">{{ label }}</option>
        </select>
        <select v-if="withSignal.includes(addType)" v-model="addSignal" aria-label="Block signal">
          <option value="">signal…</option>
          <option v-for="s in signals" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
        <input v-if="addType === 'tally'" v-model="addIndex" type="number" min="0" max="65535" placeholder="index" aria-label="Tally index" style="width: 5rem" />
        <input v-if="MS_TYPES.includes(addType)" v-model="addMs" type="number" min="0" max="86400000" placeholder="ms" aria-label="Timer milliseconds" style="width: 6rem" />
        <template v-if="addType === 'schedule'">
          <input v-model="addStart" type="number" min="0" max="1439" placeholder="start min" aria-label="Schedule start minute" style="width: 6rem" />
          <input v-model="addEnd" type="number" min="0" max="1439" placeholder="end min" aria-label="Schedule end minute" style="width: 6rem" />
        </template>
        <input v-if="addType === 'webhook'" v-model="addUrl" placeholder="https://…" aria-label="Webhook URL" style="width: 14rem" />
        <template v-if="addType === 'above'">
          <input v-model="addThreshold" type="number" min="-120" max="20" placeholder="dB" aria-label="Threshold dB" style="width: 5rem" />
          <input v-model="addHysteresis" type="number" min="0" max="60" placeholder="hyst" aria-label="Hysteresis dB" style="width: 4rem" />
        </template>
        <select v-if="addType === 'recall'" v-model="addSnapshot" aria-label="Snapshot">
          <option value="">snapshot…</option>
          <option v-for="s in snapshots" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
        <button class="btn small" @click="addBlock">+ Block</button>
      </div>

      <div class="caption" style="margin-top: 0.8rem">Wires</div>
      <div v-for="w in graph.wires" :key="wireKey(w)" class="row" :data-wire="`${w.from_block}-${w.to_block}-${w.to_port}`">
        <span>{{ w.from_block }} → {{ w.to_block }}.{{ w.to_port }}</span>
        <button class="btn small secondary" :aria-label="`Remove wire ${w.from_block} to ${w.to_block}`" @click="removeWire(w)">×</button>
      </div>
      <div class="row">
        <select v-model="wire.from" aria-label="Wire from">
          <option value="">from…</option>
          <option v-for="b in graph.blocks.filter((b) => !sinks.includes(b.type))" :key="b.id" :value="b.id">{{ b.id }}</option>
        </select>
        <select v-model="wire.to" aria-label="Wire to" @change="wire.port = portsOf(wire.to)[0] || ''">
          <option value="">to…</option>
          <option v-for="b in graph.blocks.filter((b) => inputsOf[b.type])" :key="b.id" :value="b.id">{{ b.id }}</option>
        </select>
        <select v-model="wire.port" aria-label="Wire port">
          <option v-for="p in portsOf(wire.to)" :key="p" :value="p">{{ p }}</option>
        </select>
        <button class="btn small" @click="addWire">+ Wire</button>
      </div>
    </template>
  </div>
</template>
