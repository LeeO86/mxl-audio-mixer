<script setup>
// The routing graph (§5 "graph view: a visual routing graph (read-write) for advanced users"), with Vue Flow:
// inputs → strips (their patch) → buses (sends) → monitors and outputs. Draw from a strip or bus to a bus to
// send (0 dB from its output); select a send and press Delete to remove it; click it to edit (its pan from a
// mono or stereo strip, the down-mix into a narrower bus, C1). Outputs are the published ones (Routing's
// Publish) and the virtual outputs (MIXER_OUTPUT_FLOWS, C7), drawn the same way: a node with a wire from
// its source. Draw from a strip, bus, monitor or input to a virtual output to feed it; Delete on its wire
// silences it. Click an input to name it and choose its MXL flow (C2, C3). Double-click a strip or bus to
// open its processing, a monitor for the control room (C4). A mix-minus bus shows one dotted edge from the
// bus it is taken from. Nodes are placed in columns by their routing depth; moved ones stay where they were
// put (in this browser). Node ids of inputs, monitors and outputs start with "_" (no signal id does); Vue
// Flow cannot take a ":" in an id. The view (zoom, pan, panels) survives a tab switch (App keeps it, C5).
import { computed, nextTick, ref, watch } from "vue";
import { VueFlow, useVueFlow } from "@vue-flow/core";
import "@vue-flow/core/dist/style.css";
import GraphNode from "./GraphNode.vue";
import Pot from "./Pot.vue";
import DownmixEdit from "./DownmixEdit.vue";
import { DEFAULT_DOWNMIX, FORMATS, channelsOf, tapLabel } from "../nodes.js";
import { loadTaps, tapOptions } from "../taps.js";
import { api, dbValue, fmtDb } from "../api.js";
import { act, buses, inputName, isBus, isSubgroup, live, patchSend, patchSignal, select, sendOf, signalOf, store, stored, strips } from "../store.js";
import { setVoutSource, sourceLabel, vouts } from "../vouts.js";

const emit = defineEmits(["open"]);
const { applyNodeChanges, applyEdgeChanges, fitView } = useVueFlow();
const positions = ref(JSON.parse(stored("graph.positions", "{}")));
const nodes = ref([]);
const edges = ref([]);
// One panel at a time: a send { from, to }, an input number, or a virtual output slot.
const editing = ref(null);
const inputEdit = ref(0);
const voutEdit = ref(0);
function closePanels() {
  editing.value = null;
  inputEdit.value = 0;
  voutEdit.value = 0;
}

const COL = 250;
const ROW = 78;
const isPre = (tap) => typeof tap === "string" && !["post-fader", "post-pan"].includes(tap);
const outId = (signal) => `_out_${signal.replace(/:/g, "~")}`;
const published = computed(() => (live.status?.outputs || []).filter((o) => o.virtual_output == null));
const inputs = computed(() => live.status?.inputs || []);

/** The node a virtual output's source is drawn from. */
function sourceNode(src) {
  if (!src) return null;
  if ("signal" in src) return src.signal;
  if ("monitor" in src) return `_mon_${src.monitor}`;
  return `_in_${src.input}`;
}
/** The virtual output source a node gives, or null. */
function nodeSource(id) {
  if (id.startsWith("_mon_")) return { monitor: id.slice(5) };
  if (id.startsWith("_in_")) return { input: Number(id.slice(4)) };
  return isSignal(id) ? { signal: id } : null;
}
/** What a published output is (C2): a bus, a strip's direct out or a monitor. */
function publishedNote(o) {
  if (o.signal.startsWith("monitor:")) return "monitor";
  return isBus(o.signal) ? "bus" : "direct out";
}

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
  const depth = busDepth();
  const maxDepth = Math.max(0, ...Object.values(depth));
  const out = [];
  const place = (id, col, row) => positions.value[id] || { x: col * COL, y: row * ROW };
  inputs.value.forEach((i, r) => {
    const named = inputName(i);
    const connected = i.connected_label && i.connected_label !== named ? i.connected_label : "";
    out.push({ id: `_in_${i.input}`, type: "mixer", position: place(`_in_${i.input}`, 0, r), data: { id: `_in_${i.input}`, kind: "input", label: named, note: connected ? `${connected} · ${i.state}` : i.state, out: true } });
  });
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
    out.push({ id: `_mon_${m.id}`, type: "mixer", position: place(`_mon_${m.id}`, last, r), data: { id: `_mon_${m.id}`, kind: "monitor", label: m.name, format: m.format, in: true, out: true } }),
  );
  // Outputs: the virtual ones first, then the published ones, in one column (C7).
  vouts.value.forEach((v, r) =>
    out.push({ id: `_vout_${v.slot}`, type: "mixer", position: place(`_vout_${v.slot}`, last + 1, r), data: { id: `_vout_${v.slot}`, kind: "output", label: v.label, format: v.format, note: `virtual ${v.slot} · ${v.source ? `from ${sourceLabel(v.source)}` : "silent"}`, in: true } }),
  );
  published.value.forEach((o, r) =>
    out.push({ id: outId(o.signal), type: "mixer", position: place(outId(o.signal), last + 1, vouts.value.length + r), data: { id: outId(o.signal), kind: "output", label: o.name || o.signal, format: o.format, note: `published · ${publishedNote(o)}`, in: true } }),
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
  for (const v of vouts.value) {
    const from = sourceNode(v.source);
    const tap = v.source?.tap && v.source.tap !== "post-fader" ? tapLabel(v.source.tap) : "";
    if (from) e.push({ id: `vsrc:${v.slot}`, source: from, target: `_vout_${v.slot}`, class: "pub", label: tap, data: { vout: v.slot } });
  }
  for (const o of published.value) {
    const from = o.signal.startsWith("monitor:") ? `_mon_${o.signal.slice(8)}` : o.signal;
    e.push({ id: `pub:${o.signal}`, source: from, target: outId(o.signal), class: "pub", data: { output: o.signal } });
  }
  // After Vue Flow has the nodes: it drops edges to nodes it does not know yet.
  nextTick(() => (edges.value = e));
}
watch(
  () => [live.mix, live.status?.outputs?.length, inputs.value.map((i) => `${i.label}|${i.connected_label}|${i.state}`).join(), JSON.stringify(vouts.value.map((v) => [v.label, v.source]))],
  build,
  { immediate: true, deep: false },
);

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
    } else if (edge?.data?.vout) {
      await act(() => api.del(`/api/v1/virtual-outputs/${edge.data.vout}`));
    } else if (edge?.data?.output) {
      await act(() => api.del(`/api/v1/outputs/${encodeURIComponent(edge.data.output)}`));
    }
  }
}
const isSignal = (id) => strips.value.some((s) => s.id === id) || buses.value.some((b) => b.id === id);
const isSend = (c) => isSignal(c.source) && buses.value.some((b) => b.id === c.target) && c.source !== c.target;
const isVoutFeed = (c) => typeof c.target === "string" && c.target.startsWith("_vout_") && !!nodeSource(c.source);
// Vue Flow checks the edges it is given with this too: those (with an id) are the mix's own.
const isValidConnection = (c) => !!c.id || isSend(c) || isVoutFeed(c);
async function onConnect(c) {
  if (isVoutFeed(c)) {
    const slot = Number(c.target.slice(6));
    if (await setVoutSource(slot, nodeSource(c.source))) {
      closePanels();
      voutEdit.value = slot;
    }
    return;
  }
  if (!isSend(c)) return;
  if (await act(() => api.put(`/api/v1/sends/${encodeURIComponent(c.source)}/${encodeURIComponent(c.target)}`))) {
    closePanels();
    editing.value = { from: c.source, to: c.target };
  }
}
function onEdgeClick({ edge }) {
  closePanels();
  if (edge.data?.from) editing.value = { from: edge.data.from, to: edge.data.to };
  else if (edge.data?.vout) voutEdit.value = edge.data.vout;
}
function onNodeClick({ node }) {
  if (node.id.startsWith("_in_")) {
    closePanels();
    openInput(Number(node.id.slice(4)));
  } else if (node.id.startsWith("_vout_")) {
    closePanels();
    voutEdit.value = Number(node.id.slice(6));
  }
}
// C4: a strip or bus opens its processing, a monitor the control room.
function onNodeDoubleClick({ node }) {
  if (isSignal(node.id)) {
    select(node.id);
    emit("open", { tab: "processing" });
  } else if (node.id.startsWith("_mon_")) emit("open", { tab: "control-room" });
}

// ---- a send (C1: its pan, the down-mix into a narrower bus) ----
const current = computed(() => (editing.value ? sendOf(editing.value.from, editing.value.to) : null));
const put = (body) => patchSend(editing.value.from, editing.value.to, body);
const gain = computed(() => {
  const v = dbValue(current.value?.gain_db);
  return Number.isFinite(v) ? v : -80;
});
const tapValue = computed(() => (typeof current.value?.tap === "object" ? `after:${current.value.tap.after}` : current.value?.tap));
// Only the taps the source has (G1: a bus has no input or post-pan), from the server.
watch(() => [editing.value?.from, voutEdit.value, live.revision], () => (editing.value || voutEdit.value) && loadTaps(), { immediate: true });
const tapChoices = computed(() => (current.value ? tapOptions(current.value.from, tapValue.value) : []));
function setTap(v) {
  put({ tap: v.startsWith("after:") ? { after: v.slice(6) } : v });
}
async function remove() {
  const { from, to } = editing.value;
  if (await act(() => api.del(`/api/v1/sends/${encodeURIComponent(from)}/${encodeURIComponent(to)}`))) editing.value = null;
}
const fromSig = computed(() => (current.value ? signalOf(current.value.from) : null));
const toBus = computed(() => (current.value ? signalOf(current.value.to) : null));
// A send's own surround pan (`Send.pan`, part B6: a mono or stereo source into a 5.1 or wider bus);
// else the strip's pan, which every post-pan send of it takes.
const SEND_PAN = { azimuth_deg: 0, spread_pct: 0, divergence_pct: 0, lfe_db: -100 };
const sendPan = computed(() => !!current.value?.pan && typeof current.value.pan === "object");
const canSendPan = computed(() => !sendPan.value && fromSig.value && ["mono", "stereo"].includes(fromSig.value.format) && channelsOf(toBus.value?.format) >= 6);
const setSendPan = (key, v) => put({ pan: { ...SEND_PAN, ...current.value.pan, [key]: v } });
const stripPan = computed(() => !sendPan.value && fromSig.value && !isBus(fromSig.value.id) && ["mono", "stereo"].includes(fromSig.value.format) && current.value.tap === "post-pan");
const folds = computed(() => fromSig.value && toBus.value && channelsOf(fromSig.value.format) > channelsOf(toBus.value.format));
const mixDownmix = computed(() => live.mix?.settings?.downmix || DEFAULT_DOWNMIX);
const setBusDownmix = (d) => patchSignal(toBus.value.id, { downmix: d });

// ---- an input: alias and MXL flow (C2, C3) ----
const flows = ref([]);
const inputForm = ref({ label: "", flow: "" });
const editedInput = computed(() => inputs.value.find((i) => i.input === inputEdit.value) || null);
async function openInput(n) {
  inputEdit.value = n;
  inputForm.value = { label: editedInput.value?.label || "", flow: "" };
  const list = await act(() => api.get("/api/v1/flows"));
  if (Array.isArray(list)) flows.value = list;
  const f = flows.value.find((x) => x.flow_id === editedInput.value?.flow);
  if (f) inputForm.value.flow = `${f.domain_id}|${f.flow_id}`;
}
const flowLabel = (f) => `${f.label || f.flow_id.slice(0, 8)} · ${f.channels} ch · ${f.domain_label || f.domain_id.slice(0, 8)}${f.own ? " (own)" : ""}`;
async function saveAlias() {
  const label = inputForm.value.label.trim();
  if (label && label !== editedInput.value?.label) await act(() => api.patch(`/api/v1/inputs/${inputEdit.value}`, { label }));
}
async function connectFlow() {
  const [domain_id, flow_id] = inputForm.value.flow.split("|");
  if (!flow_id || !domain_id) return;
  await act(() => api.put(`/api/v1/inputs/${inputEdit.value}/route`, { flow_id, domain_id }), `Input ${inputEdit.value} routed.`);
}
const disconnect = () => act(() => api.put(`/api/v1/inputs/${inputEdit.value}/route`, { flow_id: null }), `Input ${inputEdit.value} disconnected.`);

// ---- a virtual output: its source (C7) ----
const editedVout = computed(() => vouts.value.find((v) => v.slot === voutEdit.value) || null);
const voutTap = computed(() => {
  const t = editedVout.value?.source?.tap;
  return typeof t === "object" && t ? `after:${t.after}` : t || "post-fader";
});
const voutTapChoices = computed(() => (editedVout.value?.source && "signal" in editedVout.value.source ? tapOptions(editedVout.value.source.signal, voutTap.value) : []));
function setVoutTap(v) {
  const s = editedVout.value.source;
  setVoutSource(editedVout.value.slot, { signal: s.signal, ...(v === "post-fader" ? {} : { tap: v.startsWith("after:") ? { after: v.slice(6) } : v }) });
}
function setVoutInputFormat(f) {
  const s = editedVout.value.source;
  setVoutSource(editedVout.value.slot, { input: s.input, ...(f ? { format: f } : {}) });
}
</script>

<template>
  <div class="toolbar">
    <span class="caption">Routing graph</span>
    <span class="muted small">Draw from a strip or bus to a bus to send, or into a virtual output to feed it; select a wire and press Delete to remove it; click it to edit. Click an input for its name and flow; double-click a strip, bus or monitor to open it.</span>
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
      :zoom-on-double-click="false"
      fit-view-on-init
      class="graph"
      @nodes-change="onNodesChange"
      @edges-change="onEdgesChange"
      @node-drag-stop="onDragStop"
      @connect="onConnect"
      @edge-click="onEdgeClick"
      @node-click="onNodeClick"
      @node-double-click="onNodeDoubleClick"
    >
      <template #node-mixer="p"><GraphNode :data="p.data" /></template>
    </VueFlow>
    <div v-if="current" class="panel graphsend" data-send-edit>
      <h3>{{ fromSig?.name || current.from }} → {{ toBus?.name || current.to }}</h3>
      <p v-if="isSubgroup(current.to)" class="note" data-subgroup-lock>Into a subgroup: post-fader at 0 dB, locked (C12).</p>
      <Pot v-else label="Gain" unit="dB" :value="gain" :min="-80" :max="10" :def="0" :fine="0.5" @change="(v) => put({ gain_db: v <= -80 ? '-inf' : v })" />
      <div v-if="!isSubgroup(current.to)" class="row">
        <label>Tap</label>
        <select :value="tapValue" aria-label="Tap" @change="setTap($event.target.value)">
          <option v-for="[v, l] in tapChoices" :key="v" :value="v">{{ l }}</option>
        </select>
      </div>
      <div v-if="sendPan" data-send-pan>
        <Pot label="Send pan: position" unit="°" :value="current.pan.azimuth_deg ?? 0" :min="-180" :max="180" :def="0" :fine="0.5" :digits="0" @change="(v) => setSendPan('azimuth_deg', v)" />
        <Pot label="Send pan: spread" unit="%" :value="current.pan.spread_pct ?? 0" :min="0" :max="100" :def="0" :fine="0.5" :digits="0" @change="(v) => setSendPan('spread_pct', v)" />
        <Pot label="Send pan: divergence" unit="%" :value="current.pan.divergence_pct ?? 0" :min="0" :max="100" :def="0" :fine="0.5" :digits="0" @change="(v) => setSendPan('divergence_pct', v)" />
        <Pot label="Send pan: LFE" unit="dB (−100: off)" :value="current.pan.lfe_db ?? -100" :min="-100" :max="10" :def="-100" :fine="0.5" @change="(v) => setSendPan('lfe_db', v)" />
        <button class="btn small secondary" @click="put({ pan: null })">No send pan</button>
      </div>
      <div v-else-if="canSendPan" data-send-pan-add>
        <button class="btn small secondary" @click="put({ pan: { ...SEND_PAN } })">Surround-pan this send</button>
      </div>
      <div v-else-if="stripPan" data-strip-pan>
        <Pot :label="fromSig.format === 'mono' ? 'Pan' : 'Balance'" :value="fromSig.pan" :min="-1" :max="1" :def="0" :fine="0.01" :digits="2" @change="(v) => patchSignal(fromSig.id, { pan: v })" />
        <p class="note">The strip's pan: every post-pan send of {{ fromSig.name }} takes it.</p>
      </div>
      <div v-if="folds && toBus" class="downmixedit" data-send-downmix>
        <label>{{ fromSig.format }} → {{ toBus.format }}: down-mix into {{ toBus.name }}</label>
        <template v-if="toBus.downmix">
          <DownmixEdit :value="toBus.downmix" label="Bus down-mix" @change="setBusDownmix" />
          <button class="btn small secondary" @click="setBusDownmix(null)">Use the mix's down-mix</button>
        </template>
        <template v-else>
          <p class="note">The mix's down-mix (Settings); an own one applies to everything wider arriving at {{ toBus.name }}.</p>
          <button class="btn small secondary" @click="setBusDownmix({ ...mixDownmix })">Own down-mix</button>
        </template>
      </div>
      <div class="row">
        <button class="btn small" :class="current.mute ? 'danger' : 'secondary'" :aria-pressed="current.mute" @click="put({ mute: !current.mute })">Mute</button>
        <button class="btn small" :class="current.phase ? 'bypass' : 'secondary'" :aria-pressed="current.phase" @click="put({ phase: !current.phase })">Ø</button>
        <button class="btn small secondary" @click="remove">Remove</button>
        <button class="btn small secondary" @click="editing = null">Close</button>
      </div>
    </div>
    <div v-else-if="editedInput" class="panel graphsend" data-input-edit>
      <h3>Input {{ editedInput.input }}</h3>
      <p class="note">
        Connected: <strong>{{ editedInput.connected_label || (editedInput.flow ? "unnamed flow" : "nothing") }}</strong> · {{ editedInput.state }}<template v-if="editedInput.channels"> · {{ editedInput.channels }} ch</template>
      </p>
      <div class="row">
        <label>Alias</label>
        <input v-model="inputForm.label" aria-label="Alias" maxlength="64" @keydown.enter="saveAlias" @blur="saveAlias" />
      </div>
      <p class="note">Strips of this input still named after the old alias take the new one.</p>
      <div class="row">
        <label>Flow</label>
        <select v-model="inputForm.flow" aria-label="MXL flow">
          <option value="">–</option>
          <option v-for="f in flows" :key="`${f.domain_id}|${f.flow_id}`" :value="`${f.domain_id}|${f.flow_id}`">{{ flowLabel(f) }}</option>
        </select>
      </div>
      <div class="row">
        <button class="btn small" :disabled="!inputForm.flow" @click="connectFlow">Connect</button>
        <button class="btn small secondary" :disabled="!editedInput.flow" @click="disconnect">Disconnect</button>
        <button class="btn small secondary" @click="inputEdit = 0">Close</button>
      </div>
      <p class="note">Routes through the input's IS-05 receiver, as the designer or a crosspoint does.</p>
    </div>
    <div v-else-if="editedVout" class="panel graphsend" data-vout-edit>
      <h3>{{ editedVout.label }} <span class="muted small">virtual output {{ editedVout.slot }} · {{ editedVout.format }}</span></h3>
      <p class="note">From: <strong>{{ sourceLabel(editedVout.source) }}</strong></p>
      <div v-if="editedVout.source && 'signal' in editedVout.source" class="row">
        <label>Tap</label>
        <select :value="voutTap" aria-label="Tap" @change="setVoutTap($event.target.value)">
          <option v-for="[v, l] in voutTapChoices" :key="v" :value="v">{{ l }}</option>
        </select>
      </div>
      <div v-if="editedVout.source && 'input' in editedVout.source" class="row">
        <label>Input is</label>
        <select :value="editedVout.source.format || ''" aria-label="Input format" @change="setVoutInputFormat($event.target.value)">
          <option value="">{{ editedVout.format }} (channel for channel)</option>
          <option v-for="f in FORMATS.filter((x) => x !== editedVout.format)" :key="f" :value="f">{{ f }}</option>
        </select>
      </div>
      <p class="note">Draw a wire from a strip, bus, monitor or input into it to change the source; a wider or narrower source is down- or up-mixed. Its flow's format stays.</p>
      <div class="row">
        <button class="btn small secondary" :disabled="!editedVout.source" @click="setVoutSource(editedVout.slot, null)">Silence</button>
        <button class="btn small secondary" @click="voutEdit = 0">Close</button>
      </div>
    </div>
  </div>
</template>
