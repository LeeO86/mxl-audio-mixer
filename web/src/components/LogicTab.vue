<script setup>
// Logic graphs (§11): blocks wired together, evaluated at 100 Hz. The canvas draws the cables; the
// list under it edits the same graph. Live values are polled while the tab is open.
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { VueFlow, useVueFlow } from "@vue-flow/core";
import "@vue-flow/core/dist/style.css";
import ConfirmButton from "./ConfirmButton.vue";
import LogicNode from "./LogicNode.vue";
import { api } from "../api.js";
import { act, live, signals } from "../store.js";

const { applyNodeChanges, applyEdgeChanges, fitView } = useVueFlow({ id: "logic" });

const graphs = computed(() => live.mix?.logic || []);
const selected = ref("");
const graph = computed(() => graphs.value.find((g) => g.id === selected.value) || graphs.value[0] || null);
const name = ref("");
const values = ref({});
const buttons = ref([]);
let timer = 0;

const TYPES = [
  ["const", "Number"],
  ["button", "Button"],
  ["mute", "Mute state"],
  ["fader", "Fader"],
  ["vad", "Voice"],
  ["level", "Level"],
  ["tally", "On air"],
  ["and", "AND"],
  ["or", "OR"],
  ["not", "NOT"],
  ["compare", "Compare"],
  ["edge", "Rising edge"],
  ["latch", "Latch"],
  ["set-mute", "Set mute"],
  ["set-fader", "Set fader"],
];
const withSignal = ["mute", "fader", "vad", "level", "set-mute", "set-fader"];
const inputsOf = {
  and: ["a", "b"],
  or: ["a", "b"],
  compare: ["a", "b"],
  not: ["in"],
  edge: ["in"],
  latch: ["set", "reset"],
  "set-mute": ["in"],
  "set-fader": ["level"],
};

const addType = ref("const");
const addSignal = ref("");
const addIndex = ref("");
const wire = ref({ from: "", to: "", port: "in" });

// One change at a time, each read from the server first: two quick adds would
// otherwise each send the list they saw, and the slower one would erase the other.
let queued = Promise.resolve();
function change(mutate) {
  const id = graph.value?.id;
  if (!id) return queued;
  queued = queued.then(async () => {
    const current = await api.get(`/api/v1/logic/${id}`).catch(() => null);
    if (!current) return;
    const body = mutate(current);
    if (body) await act(() => api.patch(`/api/v1/logic/${id}`, body));
  });
  return queued;
}

async function addGraph() {
  const body = {};
  if (name.value.trim()) body.name = name.value.trim();
  const made = await act(() => api.post("/api/v1/logic", body), "Logic graph added.");
  if (made) {
    name.value = "";
    selected.value = made.id;
  }
}

function addBlock() {
  const type = addType.value;
  const signal = addSignal.value;
  const index = Math.round(Number(addIndex.value));
  if (withSignal.includes(type) && !signal) return;
  if (type === "tally" && (!Number.isInteger(index) || index < 0 || index > 65535)) return;
  change((g) => {
    const ids = new Set(g.blocks.map((b) => b.id));
    const id = [...Array(100)].map((_, i) => `${type}-${i + 1}`).find((x) => !ids.has(x));
    const block = { id, type, x: 40 + g.blocks.length * 220, y: 80 };
    if (withSignal.includes(type)) block.signal = signal;
    if (type === "tally") block.index = index;
    if (type === "const") block.value = 0;
    if (type === "compare") block.op = "gt";
    return { blocks: [...g.blocks, block] };
  });
}

function removeBlock(id) {
  change((g) => ({
    blocks: g.blocks.filter((b) => b.id !== id),
    wires: g.wires.filter((w) => w.from_block !== id && w.to_block !== id),
  }));
}

function setConst(b, value) {
  change((g) => ({ blocks: g.blocks.map((x) => (x.id === b.id ? { ...x, value: Number(value) } : x)) }));
}

function setIndex(b, index) {
  const n = Math.round(Number(index));
  if (!Number.isInteger(n) || n < 0 || n > 65535) return;
  change((g) => ({ blocks: g.blocks.map((x) => (x.id === b.id ? { ...x, index: n } : x)) }));
}

function patch(body) {
  return change(() => body);
}

function addWire() {
  const { from, to, port } = wire.value;
  if (!from || !to || !port) return;
  change((g) => {
    const wires = g.wires.filter((w) => !(w.to_block === to && w.to_port === port));
    return { wires: [...wires, { from_block: from, from_port: "out", to_block: to, to_port: port }] };
  });
}

function removeWire(w) {
  change((g) => ({
    wires: g.wires.filter((x) => !(x.from_block === w.from_block && x.to_block === w.to_block && x.to_port === w.to_port)),
  }));
}

const portsOf = (id) => inputsOf[graph.value?.blocks.find((b) => b.id === id)?.type] || [];

async function refresh() {
  const g = graph.value;
  if (!g) return;
  const d = await api.get(`/api/v1/logic/${g.id}`).catch(() => null);
  if (d) {
    values.value = d.values || {};
    buttons.value = d.buttons || [];
  }
}

function press(block, pressed) {
  const g = graph.value;
  return act(() => api.put(`/api/v1/logic/${g.id}/buttons/${block.id}`, { pressed }));
}

const nodes = ref([]);
const edges = ref([]);
const NUMERIC = new Set(["const", "fader", "level", "set-fader"]);
function shown(b) {
  const v = values.value[b.id];
  if (v === undefined || v === null) return "";
  return NUMERIC.has(b.type) ? Number(v).toFixed(1) : v >= 0.5 ? "on" : "off";
}

function labelOf(b) {
  const type = TYPES.find((t) => t[0] === b.type)?.[1] || b.type;
  if (b.type === "tally") return `${type}: ${b.index}`;
  const who = b.signal ? signals.value.find((s) => s.id === b.signal)?.name || b.signal : "";
  return who ? `${type}: ${who}` : type;
}

function rebuild() {
  const g = graph.value;
  if (!g) {
    nodes.value = [];
    edges.value = [];
    return;
  }
  nodes.value = g.blocks.map((b) => ({
    id: b.id,
    type: "logic",
    position: { x: b.x || 40, y: b.y || 40 },
    data: {
      id: b.id,
      label: labelOf(b),
      note: b.id,
      inputs: inputsOf[b.type] || [],
      output: !["set-mute", "set-fader"].includes(b.type),
      live: shown(b),
    },
  }));
  const drawn = g.wires.map((w) => ({
    id: `${w.from_block}>${w.to_block}.${w.to_port}`,
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
watch(values, () => {
  for (const n of nodes.value) {
    const b = graph.value?.blocks.find((x) => x.id === n.id);
    if (b) n.data = { ...n.data, live: shown(b), label: labelOf(b) };
  }
});

function onNodesChange(changes) {
  applyNodeChanges(changes.filter((c) => c.type !== "remove"));
}
function onDragStop({ node }) {
  change((g) => ({
    blocks: g.blocks.map((b) => (b.id === node.id ? { ...b, x: Math.round(node.position.x), y: Math.round(node.position.y) } : b)),
  }));
}
function isValidConnection(c) {
  if (c.id) return true;
  const ports = inputsOf[graph.value?.blocks.find((b) => b.id === c.target)?.type] || [];
  return c.source !== c.target && ports.includes(c.targetHandle) && (c.sourceHandle === "out" || !c.sourceHandle);
}
function onConnect(c) {
  if (!isValidConnection(c) || c.id) return;
  change((g) => {
    const wires = g.wires.filter((w) => !(w.to_block === c.target && w.to_port === c.targetHandle));
    return { wires: [...wires, { from_block: c.source, from_port: "out", to_block: c.target, to_port: c.targetHandle }] };
  });
}
function onEdgesChange(changes) {
  applyEdgeChanges(changes.filter((c) => c.type !== "remove"));
  for (const c of changes.filter((x) => x.type === "remove")) {
    const edge = edges.value.find((e) => e.id === c.id);
    if (edge?.data) removeWire(edge.data);
  }
}

onMounted(() => {
  refresh();
  timer = setInterval(refresh, 250);
});
onUnmounted(() => clearInterval(timer));
</script>

<template>
  <div class="panel">
    <h3>Logic</h3>
    <p class="muted">Blocks run 100 times a second. A loop is allowed when an edge or a latch is in it, because those remember. Actions run when their input changes.</p>
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
        <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/logic/${graph.id}`))" />
        <button class="btn small secondary" @click="fitView()">Fit</button>
      </div>

      <div class="graphwrap logiccanvas">
        <VueFlow
          id="logic"
          v-model:nodes="nodes"
          :edges="edges"
          :apply-default="false"
          :is-valid-connection="isValidConnection"
          :delete-key-code="['Delete', 'Backspace']"
          :min-zoom="0.3"
          fit-view-on-init
          class="graph"
          @nodes-change="onNodesChange"
          @edges-change="onEdgesChange"
          @node-drag-stop="onDragStop"
          @connect="onConnect"
        >
          <template #node-logic="p"><LogicNode :data="p.data" /></template>
        </VueFlow>
      </div>

      <table class="amtable" style="margin-top: 0.6rem">
        <thead>
          <tr><th>Block</th><th>Type</th><th></th><th>Value</th><th></th></tr>
        </thead>
        <tbody>
          <tr v-for="b in graph.blocks" :key="b.id" :data-block="b.id">
            <td>{{ b.id }}</td>
            <td>{{ TYPES.find((t) => t[0] === b.type)?.[1] || b.type }}</td>
            <td>
              <template v-if="b.type === 'const'">
                <input type="number" :value="b.value" :aria-label="`${b.id} value`" style="width: 5rem" @change="setConst(b, $event.target.value)" />
              </template>
              <template v-else-if="b.type === 'tally'">
                <input type="number" min="0" max="65535" :value="b.index" :aria-label="`${b.id} index`" style="width: 5rem" @change="setIndex(b, $event.target.value)" />
              </template>
              <template v-else-if="b.signal">{{ signals.find((s) => s.id === b.signal)?.name || b.signal }}</template>
              <button v-else-if="b.type === 'button'" class="btn small" :class="{ primary: buttons.includes(b.id) }" :aria-pressed="buttons.includes(b.id)" @click="press(b, !buttons.includes(b.id))">
                {{ buttons.includes(b.id) ? "Pressed" : "Press" }}
              </button>
            </td>
            <td class="num" :data-value="b.id">{{ shown(b) }}</td>
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
        <button class="btn small" @click="addBlock">+ Block</button>
      </div>

      <div class="caption" style="margin-top: 0.8rem">Wires</div>
      <div v-for="(w, i) in graph.wires" :key="i" class="row" :data-wire="`${w.from_block}-${w.to_block}-${w.to_port}`">
        <span>{{ w.from_block }} → {{ w.to_block }}.{{ w.to_port }}</span>
        <button class="btn small secondary" :aria-label="`Remove wire ${w.from_block} to ${w.to_block}`" @click="removeWire(w)">×</button>
      </div>
      <div class="row">
        <select v-model="wire.from" aria-label="Wire from">
          <option value="">from…</option>
          <option v-for="b in graph.blocks.filter((b) => !['set-mute', 'set-fader'].includes(b.type))" :key="b.id" :value="b.id">{{ b.id }}</option>
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
