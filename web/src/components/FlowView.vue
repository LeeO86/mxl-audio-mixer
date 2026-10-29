<script setup>
// The flow view (B4) at the bottom of the processing view: the signal flow of one strip or bus from left to
// right (input, trim, the chain's nodes, fader, pan, out) with its endpoints in the gaps where they are
// taken: sends to buses (bus outs), the published output (a strip's direct out, a bus out), and keys other
// nodes take from here (sidechain out); a keyed node shows where its key comes from (sidechain in). Drag a
// node onto a gap to move it (the chain order); drag an endpoint onto a gap to take it there (its tap). The
// same endpoints are in the mix (sends, outputs[].tap, chain keys), so the Graph view shows them too.
import { computed, ref, watch } from "vue";
import { NODE_TYPES, tapLabel } from "../nodes.js";
import { api } from "../api.js";
import { act, buses, flush, isBus, live, patchNode, patchSend, signalOf, signalPath, signals } from "../store.js";

const props = defineProps({ signal: { type: Object, required: true } });

const bus = computed(() => isBus(props.signal.id));
const chain = computed(() => props.signal.chain || []);
const CATEGORY = { eq: "eq", filter: "eq", "dynamic-eq": "eq", delay: "other", oscillator: "other", auto: "other", width: "other", "surround-pan": "other" };
const category = (n) => CATEGORY[n.type] || "dynamics";
function span(cat) {
  const idx = chain.value.map((n, i) => (category(n) === cat ? i : -1)).filter((i) => i >= 0);
  return idx.length ? [idx[0], idx[idx.length - 1] + 1] : [0, 0];
}

// Gaps: "in" (before the trim, strips), 0 … n (before node k; n: after the chain, pre-fader), "fader"
// (post-fader), "pan" (post-pan, strips).
const gapOf = (tap) => {
  if (typeof tap === "object" && tap?.after) {
    const i = chain.value.findIndex((n) => n.id === tap.after);
    return i < 0 ? 0 : i + 1;
  }
  const [fe, le] = span("eq");
  const [fd, ld] = span("dynamics");
  return { input: "in", "pre-eq": fe, "post-eq": le, "pre-dynamics": fd, "post-dynamics": ld, "pre-fader": chain.value.length, "post-fader": "fader", "post-pan": "pan" }[tap] ?? "fader";
};
/** The tap that a drop on gap `g` sets (null: none there). */
function tapAt(g) {
  if (g === "in") return "input";
  if (g === "fader") return "post-fader";
  if (g === "pan") return "post-pan";
  if (g === chain.value.length) return "pre-fader";
  if (g > 0) return { after: chain.value[g - 1].id };
  if (span("eq")[0] === 0) return "pre-eq";
  if (span("dynamics")[0] === 0) return "pre-dynamics";
  return null;
}
const sameTap = (a, b) => JSON.stringify(a) === JSON.stringify(b);

// The endpoints of this signal, by gap.
const output = computed(() => (live.mix?.outputs || []).find((o) => o.signal === props.signal.id) || null);
const endpoints = computed(() => {
  const id = props.signal.id;
  const out = [];
  for (const s of live.mix?.sends || []) if (s.from === id) out.push({ kind: "send", key: `send:${s.to}`, label: `→ ${signalOf(s.to)?.name || s.to}`, title: `Send to ${signalOf(s.to)?.name || s.to} (bus out), ${tapLabel(s.tap)}`, tap: s.tap, to: s.to });
  if (output.value) {
    const tap = output.value.tap || "post-fader";
    out.push({ kind: "output", key: "output", label: bus.value ? "Bus out" : "Direct out", title: `Published (MXL flow), ${tapLabel(tap)}`, tap });
  }
  for (const s of signals.value)
    for (const n of s.chain || [])
      if (n.key?.signal === id) out.push({ kind: "key", key: `key:${s.id}:${n.id}`, label: `key → ${s.name}`, title: `Sidechain out: the key of ${s.name} / ${n.id}, ${tapLabel(n.key.tap)}`, tap: n.key.tap, owner: s.id, node: n.id });
  return out;
});
const at = (g) => endpoints.value.filter((e) => gapOf(e.tap) === g);

// The flow, left to right: stages (fixed ones and the nodes) with a gap between each two.
const stages = computed(() => {
  const st = bus.value ? [{ label: "Sum" }] : [{ label: "Input" }, { gap: "in" }, { label: "Trim" }];
  chain.value.forEach((n, i) => st.push({ gap: i }, { node: n, i }));
  st.push({ gap: chain.value.length }, { label: "Fader" }, { gap: "fader" });
  if (!bus.value) st.push({ label: chain.value.some((n) => n.type === "surround-pan" && !n.bypass) ? "Surround pan" : "Pan" }, { gap: "pan" });
  st.push({ label: "Out" });
  return st;
});

// Dragging (HTML drag and drop): "node:<id>" or "ep:<key>".
const dragging = ref("");
const over = ref(null);
function start(ev, what) {
  dragging.value = what;
  ev.dataTransfer.setData("text/plain", what);
  ev.dataTransfer.effectAllowed = "move";
}
function end() {
  dragging.value = "";
  over.value = null;
}
// Where a node can go: the gaps inside the chain; an endpoint: every gap with a tap.
function accepts(g) {
  if (dragging.value.startsWith("node:")) return typeof g === "number";
  return dragging.value.startsWith("ep:") && tapAt(g) !== null;
}
function enter(ev, g) {
  if (!accepts(g)) return;
  ev.preventDefault();
  over.value = g;
}
async function drop(ev, g) {
  const what = dragging.value || ev.dataTransfer.getData("text/plain");
  end();
  if (what.startsWith("node:")) {
    const id = what.slice(5);
    const from = chain.value.findIndex((n) => n.id === id);
    const to = g > from ? g - 1 : g;
    if (from >= 0 && to !== from) {
      flush();
      await act(() => api.post(`${signalPath(props.signal.id)}/chain/${encodeURIComponent(id)}/move`, { position: to }));
    }
    return;
  }
  const e = endpoints.value.find((x) => `ep:${x.key}` === what);
  const tap = tapAt(g);
  if (!e || tap === null || gapOf(e.tap) === g || sameTap(e.tap, tap)) return;
  if (e.kind === "send") patchSend(props.signal.id, e.to, { tap });
  else if (e.kind === "key") patchNode(e.owner, e.node, { key: { signal: props.signal.id, tap } });
  else if (e.kind === "output") {
    const o = output.value;
    await act(() => api.put(`/api/v1/outputs/${encodeURIComponent(props.signal.id)}`, { tap, format: o.format ?? null, commit_samples: o.commit_samples ?? null }));
  }
}

// Adding an endpoint: a send to a bus or the published output, at the source's default tap.
const adding = ref("");
const free = computed(() => buses.value.filter((b) => b.id !== props.signal.id && !(live.mix?.sends || []).some((s) => s.from === props.signal.id && s.to === b.id)));
async function add(v) {
  adding.value = "";
  if (v === "output") await act(() => api.put(`/api/v1/outputs/${encodeURIComponent(props.signal.id)}`), `${props.signal.name} published.`);
  else if (v) await act(() => api.put(`/api/v1/sends/${encodeURIComponent(props.signal.id)}/${encodeURIComponent(v)}`));
}
watch(() => props.signal.id, end);

// A click on a node shows its tile above.
function reveal(i) {
  document.querySelector(`.nodecard[data-drop="${i}"]`)?.scrollIntoView({ behavior: "smooth", block: "center" });
}
const keyOf = (n) => (n.key ? `${signalOf(n.key.signal)?.name || n.key.signal} · ${tapLabel(n.key.tap)}` : "");
</script>

<template>
  <div class="panel flowview" data-flow-view>
    <h3>
      Signal flow
      <span class="spacer"></span>
      <select :value="adding" class="addep" aria-label="Add an endpoint" @change="add($event.target.value)">
        <option value="">+ endpoint…</option>
        <option v-if="!output" value="output">{{ bus ? "Bus out" : "Direct out" }} (publish)</option>
        <option v-for="b in free" :key="b.id" :value="b.id">Send to {{ b.name }}</option>
      </select>
    </h3>
    <div class="flow" @dragend="end">
      <template v-for="(s, k) in stages" :key="k">
        <div v-if="s.gap !== undefined" class="gap" :class="{ over: over === s.gap, can: dragging && accepts(s.gap) }" :data-gap="s.gap" @dragenter="enter($event, s.gap)" @dragover="enter($event, s.gap)" @dragleave="over === s.gap && (over = null)" @drop.prevent="drop($event, s.gap)">
          <span class="wire"></span>
          <span v-for="e in at(s.gap)" :key="e.key" class="ep" :class="e.kind" draggable="true" :title="`${e.title}: drag to take it elsewhere`" :data-endpoint="e.key" @dragstart="start($event, `ep:${e.key}`)">{{ e.label }}</span>
        </div>
        <div v-if="s.node" class="stage node" :class="{ bypassed: s.node.bypass }" draggable="true" :data-flow-node="s.node.id" :title="`${s.node.id}: drag onto a gap to move it, click to show it`" @dragstart="start($event, `node:${s.node.id}`)" @click="reveal(s.i)">
          <span>{{ NODE_TYPES[s.node.type]?.label || s.node.type }}</span>
          <code>{{ s.node.id }}</code>
          <span v-if="s.node.key" class="keyin" :title="`Sidechain in: the key from ${keyOf(s.node)}`">key ← {{ keyOf(s.node) }}</span>
        </div>
        <div v-else-if="s.label" class="stage fixed">{{ s.label }}</div>
      </template>
    </div>
    <p class="note">Drag a node onto a gap to move it; drag an endpoint (send, {{ bus ? "bus out" : "direct out" }}, key) onto a gap to take it there.</p>
  </div>
</template>

<style scoped>
.addep {
  width: auto;
}
.flow {
  display: flex;
  align-items: stretch;
  overflow-x: auto;
  padding: 0.3rem 0 0.5rem;
}
.stage {
  flex: none;
  display: flex;
  flex-direction: column;
  justify-content: center;
  align-items: center;
  gap: 0.15rem;
  min-width: 4.5rem;
  padding: 0.35rem 0.5rem;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--panel2);
  font-size: 0.85rem;
}
.stage.node {
  cursor: grab;
  border-color: var(--accent);
}
.stage.node.bypassed {
  opacity: 0.5;
  border-style: dashed;
}
.stage.fixed {
  color: var(--muted);
}
.stage code {
  font-size: 0.7rem;
}
.keyin {
  font-size: 0.7rem;
  color: var(--warn);
}
.gap {
  flex: none;
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: flex-start;
  gap: 0.2rem;
  min-width: 1.6rem;
  padding: 1.6rem 0.2rem 0.2rem;
  border-radius: 4px;
}
.gap .wire {
  position: absolute;
  top: 1.1rem;
  left: 0;
  right: 0;
  height: 2px;
  background: var(--border);
}
.gap.can {
  background: color-mix(in srgb, var(--accent) 10%, transparent);
}
.gap.over {
  background: color-mix(in srgb, var(--accent) 30%, transparent);
}
.ep {
  cursor: grab;
  white-space: nowrap;
  font-size: 0.72rem;
  padding: 0.05rem 0.35rem;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: var(--panel);
}
.ep.send {
  border-color: var(--accent);
}
.ep.output {
  border-color: var(--ok);
}
.ep.key {
  border-color: var(--warn);
}
</style>
