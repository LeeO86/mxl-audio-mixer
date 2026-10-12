<script setup>
// The routing graph (§5 "graph view: a visual routing graph (read-write) for advanced users"), with Vue Flow:
// inputs → strips (their patch) → buses (sends) → monitors and published outputs. Draw from a strip or bus
// to a bus to send (0 dB from its output); select a send and press Delete to remove it; click it to edit.
// A mix-minus bus shows one dotted edge from the bus it is taken from. Nodes are placed in columns by their
// routing depth; moved ones stay where they were put (in this browser). Node ids of inputs, monitors and
// outputs start with "_" (no signal id does); Vue Flow cannot take a ":" in an id.
import { computed, nextTick, ref, watch } from "vue";
import { VueFlow, useVueFlow } from "@vue-flow/core";
import "@vue-flow/core/dist/style.css";
import GraphNode from "./GraphNode.vue";
import Pot from "./Pot.vue";
import { TAPS, tapLabel } from "../nodes.js";
import { api, dbValue, fmtDb } from "../api.js";
import { act, buses, live, patchSend, sendOf, signalOf, store, stored, strips } from "../store.js";

const { applyNodeChanges, applyEdgeChanges, fitView } = useVueFlow();
const positions = ref(JSON.parse(stored("graph.positions", "{}")));
const nodes = ref([]);
const edges = ref([]);
const editing = ref(null); // { from, to }

const COL = 250;
const ROW = 78;
const isPre = (tap) => typeof tap === "string" && !["post-fader", "post-pan"].includes(tap);

/** The column of each bus: one more than the deepest bus that sends to it. */
function busDepth() {
  const depth = {};
  const sendsTo = (b) => (live.mix?.sends || []).filter((s) => s.to === b && !s.block_delay).map((s) => s.from);
  const visit = (id, seen) => {
    if (depth[id] !== undefined) return depth[id];
    if (seen.has(id)) return 0;
    seen.add(id);
    const bus = buses.value.find((x) => x.id === id);
    const from = [...sendsTo(id), ...(bus?.minus ? [bus.minus.of] : [])].filter((f) => buses.value.some((x) => x.id === f));
    depth[id] = from.length ? 1 + Math.max(...from.map((f) => visit(f, seen))) : 0;
    return depth[id];
  };
  for (const b of buses.value) visit(b.id, new Set());
  return depth;
}

function build() {
  const mix = live.mix;
  if (!mix) return;
  const inputs = live.status?.inputs || [];
  const depth = busDepth();
  const maxDepth = Math.max(0, ...Object.values(depth));
  const out = [];
  const place = (id, col, row) => positions.value[id] || { x: col * COL, y: row * ROW };
  inputs.forEach((i, r) =>
    out.push({ id: `_in_${i.input}`, type: "mixer", position: place(`_in_${i.input}`, 0, r), data: { id: `_in_${i.input}`, kind: "input", label: i.label || `Input ${i.input}`, note: i.state, out: true } }),
  );
  strips.value.forEach((s, r) =>
    out.push({ id: s.id, type: "mixer", position: place(s.id, 1, r), data: { id: s.id, kind: "strip", label: s.name, format: s.format, colour: s.colour, muted: s.mute, in: true, out: true } }),
  );
  const rows = {};
  for (const b of buses.value) {
    const col = 2 + (depth[b.id] || 0);
    const r = (rows[col] = (rows[col] ?? -1) + 1);
    out.push({ id: b.id, type: "mixer", position: place(b.id, col, r), data: { id: b.id, kind: "bus", label: b.name, format: b.format, colour: b.colour, muted: b.mute, note: b.minus ? `− ${signalOf(b.minus.of)?.name || b.minus.of}` : "", in: true, out: true } });
  }
  const last = 3 + maxDepth;
  (mix.control_room?.monitors || []).forEach((m, r) =>
    out.push({ id: `_mon_${m.id}`, type: "mixer", position: place(`_mon_${m.id}`, last, r), data: { id: `_mon_${m.id}`, kind: "monitor", label: m.name, format: m.format, in: true, out: m.publish } }),
  );
  const published = live.status?.outputs || [];
  published.forEach((o, r) =>
    out.push({ id: `_out_${o.signal}`, type: "mixer", position: place(`_out_${o.signal}`, last + 1, r), data: { id: `_out_${o.signal}`, kind: "output", label: o.name || o.signal, format: o.format, in: true } }),
  );
  nodes.value = out;

  const e = [];
  for (const s of strips.value) {
    s.patch.forEach((p, ch) => {
      if (p && "input" in p) e.push({ id: `patch:${s.id}:${ch}`, source: `_in_${p.input}`, target: s.id, class: "patch", label: `${p.channel}`, deletable: false, selectable: false });
    });
  }
  for (const x of mix.sends || []) {
    const gain = dbValue(x.gain_db);
    e.push({
      id: `send:${x.from}:${x.to}`,
      source: x.from,
      target: x.to,
      class: ["send", isPre(x.tap) ? "pre" : "post", x.mute ? "off" : ""].join(" "),
      label: `${x.mute ? "muted" : Number.isFinite(gain) ? fmtDb(gain) : "−∞"}${isPre(x.tap) ? ` ${tapLabel(x.tap)}` : ""}`,
      data: { from: x.from, to: x.to },
    });
  }
  for (const b of buses.value.filter((x) => x.minus)) {
    e.push({ id: `minus:${b.id}`, source: b.minus.of, target: b.id, class: "minus", label: `minus ${b.minus.without.map((w) => signalOf(w)?.name || w).join(", ")}`, deletable: false });
  }
  for (const m of mix.control_room?.monitors || []) {
    if (m.source && "signal" in m.source) e.push({ id: `msrc:${m.id}`, source: m.source.signal, target: `_mon_${m.id}`, class: "monsrc", deletable: false });
    if (m.source && "input" in m.source) e.push({ id: `msrc:${m.id}`, source: `_in_${m.source.input}`, target: `_mon_${m.id}`, class: "monsrc", deletable: false });
  }
  for (const o of published) {
    const from = o.signal.startsWith("monitor:") ? `_mon_${o.signal.slice(8)}` : o.signal;
    e.push({ id: `pub:${o.signal}`, source: from, target: `_out_${o.signal}`, class: "pub", data: { output: o.signal } });
  }
  // After Vue Flow has the nodes: it drops edges to nodes it does not know yet.
  nextTick(() => (edges.value = e));
}
watch(() => [live.mix, live.status?.outputs?.length, live.status?.inputs?.length], build, { immediate: true, deep: false });

function onNodesChange(changes) {
  applyNodeChanges(changes.filter((c) => c.type !== "remove"));
}
function onDragStop({ node }) {
  positions.value = { ...positions.value, [node.id]: node.position };
  store("graph.positions", JSON.stringify(positions.value));
}
function resetLayout() {
  positions.value = {};
  store("graph.positions", "{}");
  build();
  setTimeout(() => fitView(), 50);
}
async function onEdgesChange(changes) {
  applyEdgeChanges(changes.filter((c) => c.type !== "remove"));
  for (const c of changes.filter((x) => x.type === "remove")) {
    const edge = edges.value.find((x) => x.id === c.id);
    if (edge?.data?.from) {
      await act(() => api.del(`/api/v1/sends/${encodeURIComponent(edge.data.from)}/${encodeURIComponent(edge.data.to)}`));
      if (editing.value?.from === edge.data.from && editing.value?.to === edge.data.to) editing.value = null;
    } else if (edge?.data?.output) {
      await act(() => api.del(`/api/v1/outputs/${encodeURIComponent(edge.data.output)}`));
    }
  }
}
const isSignal = (id) => strips.value.some((s) => s.id === id) || buses.value.some((b) => b.id === id);
// Vue Flow checks the edges it is given with this too: those (with an id) are the mix's own.
const isValidConnection = (c) => !!c.id || (isSignal(c.source) && buses.value.some((b) => b.id === c.target) && c.source !== c.target);
async function onConnect(c) {
  if (!isValidConnection(c)) return;
  if (await act(() => api.put(`/api/v1/sends/${encodeURIComponent(c.source)}/${encodeURIComponent(c.target)}`))) editing.value = { from: c.source, to: c.target };
}
function onEdgeClick({ edge }) {
  editing.value = edge.data?.from ? { from: edge.data.from, to: edge.data.to } : null;
}

const current = computed(() => (editing.value ? sendOf(editing.value.from, editing.value.to) : null));
const put = (body) => patchSend(editing.value.from, editing.value.to, body);
const gain = computed(() => {
  const v = dbValue(current.value?.gain_db);
  return Number.isFinite(v) ? v : -80;
});
const tapValue = computed(() => (typeof current.value?.tap === "object" ? `after:${current.value.tap.after}` : current.value?.tap));
function setTap(v) {
  put({ tap: v.startsWith("after:") ? { after: v.slice(6) } : v });
}
async function remove() {
  const { from, to } = editing.value;
  if (await act(() => api.del(`/api/v1/sends/${encodeURIComponent(from)}/${encodeURIComponent(to)}`))) editing.value = null;
}
</script>

<template>
  <div class="toolbar">
    <span class="caption">Routing graph</span>
    <span class="muted small">Draw from a strip or bus to a bus to send; select a send and press Delete to remove it; click it to edit.</span>
    <span class="spacer"></span>
    <button class="btn secondary" @click="fitView()">Fit</button>
    <button class="btn secondary" @click="resetLayout">Arrange</button>
  </div>
  <div class="graphwrap">
    <VueFlow
      v-model:nodes="nodes"
:edges="edges"
      :apply-default="false"
      :is-valid-connection="isValidConnection"
      :delete-key-code="['Delete', 'Backspace']"
      :min-zoom="0.2"
      fit-view-on-init
      class="graph"
      @nodes-change="onNodesChange"
      @edges-change="onEdgesChange"
      @node-drag-stop="onDragStop"
      @connect="onConnect"
      @edge-click="onEdgeClick"
    >
      <template #node-mixer="p"><GraphNode :data="p.data" /></template>
    </VueFlow>
    <div v-if="current" class="panel sendedit" data-send-edit>
      <h3>{{ signalOf(current.from)?.name || current.from }} → {{ signalOf(current.to)?.name || current.to }}</h3>
      <Pot label="Gain" unit="dB" :value="gain" :min="-80" :max="10" :def="0" :fine="0.5" @change="(v) => put({ gain_db: v <= -80 ? '-inf' : v })" />
      <div class="row">
        <label>Tap</label>
        <select :value="tapValue" aria-label="Tap" @change="setTap($event.target.value)">
          <option v-for="[v, l] in TAPS" :key="v" :value="v">{{ l }}</option>
          <option v-for="n in signalOf(current.from)?.chain || []" :key="n.id" :value="`after:${n.id}`">after {{ n.id }}</option>
        </select>
      </div>
      <div class="row">
        <button class="btn small" :class="current.mute ? 'danger' : 'secondary'" :aria-pressed="current.mute" @click="put({ mute: !current.mute })">Mute</button>
        <button class="btn small" :class="current.phase ? 'bypass' : 'secondary'" :aria-pressed="current.phase" @click="put({ phase: !current.phase })">Ø</button>
        <button class="btn small secondary" @click="remove">Remove</button>
        <button class="btn small secondary" @click="editing = null">Close</button>
      </div>
    </div>
  </div>
</template>
