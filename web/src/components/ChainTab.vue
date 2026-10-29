<script setup>
// The processing chain editor (§12.2): pick a strip or bus (◀ ▶ or the [ and ] keys step through them);
// drag a node type from the palette into the chain (or click it to append), drag nodes by their header to
// reorder them (or use ← →), bypass, delete (press twice), and edit each node. The nodes are tiles of one
// size in a grid, in chain order (left to right, then down); a click on a tile's header opens or closes
// it. Dragging uses pointer events, so the wheel scrolls meanwhile; near the window's top or bottom
// edge it scrolls by itself. The palette stays in view. The chain's response is drawn on top.
// Copy puts a unit on the clipboard (this browser, B1); on any chain the clipboard bar pastes all of it,
// or one unit (appended), and a unit's Paste takes the last copied unit of its type (parameters only).
// The flow view at the bottom shows the signal flow with its endpoints (B4).
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import ChainCurve from "./ChainCurve.vue";
import SignalSettings from "./SignalSettings.vue";
import ConfirmButton from "./ConfirmButton.vue";
import NodeEditor from "./NodeEditor.vue";
import FormatSelect from "./FormatSelect.vue";
import FlowView from "./FlowView.vue";
import { clear, clipboard, copy, forget, latestOf, replacePatch } from "../clipboard.js";
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

// The previous and next strip or bus (strips first, then buses, as in the signal list).
const at = computed(() => signals.value.findIndex((s) => s.id === sig.value?.id));
const prev = computed(() => (at.value > 0 ? signals.value[at.value - 1] : null));
const next = computed(() => (at.value >= 0 && at.value < signals.value.length - 1 ? signals.value[at.value + 1] : null));
const step = (s) => s && select(s.id);
// [ and ] step too, unless a text field, number or list has the keyboard.
function onKey(ev) {
  if (ev.ctrlKey || ev.metaKey || ev.altKey || (ev.key !== "[" && ev.key !== "]")) return;
  const t = ev.target;
  if (t?.closest?.('textarea, select, [contenteditable="true"], input:not([type="range"]):not([type="checkbox"]):not([type="radio"])')) return;
  ev.preventDefault();
  step(ev.key === "[" ? prev.value : next.value);
}
onMounted(() => window.addEventListener("keydown", onKey));
onUnmounted(() => {
  window.removeEventListener("keydown", onKey);
  endDrag();
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

// Copy and paste (B1).
const copied = ref("");
function copyNode(n) {
  copy(n, sig.value.id);
  copied.value = n.id;
  setTimeout(() => copied.value === n.id && (copied.value = ""), 1200);
}
const nameOf = (id) => signals.value.find((s) => s.id === id)?.name || id;
const unitLabel = (u) => NODE_TYPES[u.unit.type]?.label || u.unit.type;
// All or nothing: when the server refuses a unit (a limit, a key to itself …), the units this paste
// added are removed again and its message stays.
async function pasteUnits(units) {
  const path = `${signalPath(sig.value.id)}/chain`;
  const added = [];
  for (const u of units) {
    try {
      added.push((await api.post(path, JSON.parse(JSON.stringify(u.unit)))).id);
    } catch (e) {
      for (const id of added.reverse()) await api.del(`${path}/${encodeURIComponent(id)}`).catch(() => {});
      live.actionError = `Paste: ${e.message}${added.length ? " (nothing pasted)" : ""}`;
      return;
    }
  }
  live.actionError = "";
}
const pasteAll = () => pasteUnits(clipboard.units);
async function pasteOnto(n) {
  const u = latestOf(n.type);
  if (!u) return;
  flush();
  await act(() => api.patch(`${signalPath(sig.value.id)}/chain/${encodeURIComponent(n.id)}`, replacePatch(n, u.unit)));
}

// Dragging with pointer events: a palette chip carries "type:<t>", a tile's header "node:<id>". It
// starts after 6 px of movement; less is a click. The drop target is the tile under the pointer (insert
// before it) or the drop zone at the end (append), found again when the page scrolls under the pointer.
const drag = reactive({ what: "", label: "", active: false, x: 0, y: 0, over: -1 });
let start = null; // { x, y }
let justDragged = false;
let scroller = 0;
const INTERACTIVE = "button, input, select, textarea, a, label";

function pointerDown(ev, what, label) {
  if (ev.button !== 0 || ev.currentTarget.disabled || (what.startsWith("node:") && ev.target.closest(INTERACTIVE))) return;
  Object.assign(drag, { what, label, active: false, x: ev.clientX, y: ev.clientY, over: -1 });
  start = { x: ev.clientX, y: ev.clientY };
  window.addEventListener("pointermove", pointerMove);
  window.addEventListener("pointerup", pointerUp);
  window.addEventListener("pointercancel", endDrag);
  window.addEventListener("keydown", escape, true);
  window.addEventListener("scroll", findOver, true);
}
// Escape cancels a drag; the release after it is no click either.
function escape(ev) {
  if (ev.key !== "Escape") return;
  if (drag.active) {
    ev.preventDefault();
    window.addEventListener(
      "pointerup",
      () => {
        justDragged = true;
        setTimeout(() => (justDragged = false), 0);
      },
      { once: true, capture: true },
    );
  }
  endDrag();
}
function pointerMove(ev) {
  drag.x = ev.clientX;
  drag.y = ev.clientY;
  if (!drag.active) {
    if (Math.hypot(ev.clientX - start.x, ev.clientY - start.y) < 6) return;
    drag.active = true;
    document.body.classList.add("dragging-node");
    scroller = requestAnimationFrame(autoScroll);
  }
  findOver();
}
function findOver() {
  if (!drag.active) return;
  const el = document.elementFromPoint(drag.x, drag.y)?.closest("[data-drop]");
  drag.over = el ? Number(el.dataset.drop) : -1;
}
// Near the top or bottom edge of the window, scroll (faster closer to the edge).
function autoScroll() {
  if (!drag.active) return;
  const edge = 60;
  const h = window.innerHeight;
  const dy = drag.y < edge ? -(edge - drag.y) / 3 : drag.y > h - edge ? (drag.y - (h - edge)) / 3 : 0;
  if (dy) window.scrollBy(0, dy);
  scroller = requestAnimationFrame(autoScroll);
}
function pointerUp() {
  const { what, active, over } = drag;
  endDrag();
  if (!active) return;
  justDragged = true;
  setTimeout(() => (justDragged = false), 0);
  if (over >= 0) drop(what, over);
}
function endDrag() {
  window.removeEventListener("pointermove", pointerMove);
  window.removeEventListener("pointerup", pointerUp);
  window.removeEventListener("pointercancel", endDrag);
  window.removeEventListener("keydown", escape, true);
  window.removeEventListener("scroll", findOver, true);
  cancelAnimationFrame(scroller);
  document.body.classList.remove("dragging-node");
  Object.assign(drag, { what: "", label: "", active: false, over: -1 });
}
function drop(what, i) {
  if (what.startsWith("type:")) add(what.slice(5), i);
  else if (what.startsWith("node:")) {
    const id = what.slice(5);
    const from = chain.value.findIndex((n) => n.id === id);
    if (from < 0) return;
    const to = i > from ? i - 1 : i;
    if (to !== from) move(id, to);
  }
}
// A click (not the end of a drag) on a palette chip appends; on a tile's header (not on its buttons)
// opens or closes the tile.
function chipClick(type) {
  if (!justDragged) add(type);
}
function headClick(ev, id) {
  if (justDragged || ev.target.closest(INTERACTIVE)) return;
  open.value[id] = !isOpen(id);
}
const chainCurve = computed(() => chain.value.filter((n) => !n.bypass && ["eq", "filter", "dynamic-eq"].includes(n.type)));
</script>

<template>
  <div v-if="!signals.length" class="empty">No strips or buses yet.</div>
  <div v-else class="chainlayout">
    <div class="chainside">
      <div class="panel palette">
        <h3>Add</h3>
        <p class="note" style="margin-top: 0">Drag into the chain, or click to append.</p>
        <template v-for="(items, g) in groups" :key="g">
          <div class="group-caption">{{ g }}</div>
          <button v-for="t in items" :key="t.type" class="pchip" :title="t.help" :disabled="chain.length >= 32 || (t.type === 'auto' && chain.some((n) => n.type === 'auto'))" @pointerdown="pointerDown($event, `type:${t.type}`, t.label)" @click="chipClick(t.type)">
            {{ t.label }} <span class="muted">+</span>
          </button>
        </template>
      </div>
    </div>

    <div>
      <div class="panel chainhead">
        <button class="btn secondary" :disabled="!prev" :title="prev ? `${prev.name} ( [ )` : ''" aria-label="Previous strip or bus" @click="step(prev)">◀</button>
        <select :value="sig?.id" aria-label="Signal" @change="select($event.target.value)">
          <optgroup label="Strips">
            <option v-for="s in live.mix.strips" :key="s.id" :value="s.id">{{ s.name }}</option>
          </optgroup>
          <optgroup label="Buses">
            <option v-for="b in live.mix.buses" :key="b.id" :value="b.id">{{ b.name }}</option>
          </optgroup>
        </select>
        <button class="btn secondary" :disabled="!next" :title="next ? `${next.name} ( ] )` : ''" aria-label="Next strip or bus" @click="step(next)">▶</button>
        <span class="note">{{ chain.length }} of 32 nodes · latency {{ (live.latency[sig?.id] || 0).toFixed(2) }} ms</span>
        <FormatSelect v-if="sig" :signal="sig" />
        <span class="spacer"></span>
        <span class="muted small">[ and ]: previous, next</span>
      </div>
      <div v-if="clipboard.units.length" class="panel clipbar" role="group" aria-label="Clipboard">
        <span class="muted small">Clipboard</span>
        <span v-for="(u, i) in clipboard.units" :key="i" class="clipchip">
          <button class="btn small secondary" :title="`Paste ${unitLabel(u)} (copied from ${nameOf(u.from)}, ${u.label}) at the end of this chain`" @click="pasteUnits([u])">{{ unitLabel(u) }} <span class="muted">· {{ nameOf(u.from) }}</span></button>
          <button class="btn small secondary" :aria-label="`Remove ${unitLabel(u)} from the clipboard`" @click="forget(i)">✕</button>
        </span>
        <span class="spacer"></span>
        <button class="btn small" :disabled="chain.length + clipboard.units.length > 32" @click="pasteAll">Paste all</button>
        <button class="btn small secondary" @click="clear">Clear</button>
      </div>
      <div class="chaintop">
        <SignalSettings v-if="sig" :signal="sig" />
        <div v-if="chainCurve.length" class="panel">
          <h3>{{ sig.name }}: chain response</h3>
          <ChainCurve :chain="chainCurve" />
        </div>
      </div>
      <div class="nodes">
        <div v-for="(n, i) in chain" :key="n.id" :data-drop="i" class="nodecard" :class="{ bypassed: n.bypass, over: drag.active && drag.over === i, dragging: drag.active && drag.what === `node:${n.id}`, closed: !isOpen(n.id) }">
          <div class="nhead" :title="isOpen(n.id) ? 'click to close, drag to move' : 'click to open, drag to move'" @pointerdown="pointerDown($event, `node:${n.id}`, NODE_TYPES[n.type]?.label || n.type)" @click="headClick($event, n.id)">
            <span class="ntitle">{{ NODE_TYPES[n.type]?.label || n.type }}</span>
            <code>{{ n.id }}</code>
            <span v-if="unsupported(n, sig.format, !isBus(sig.id))" class="state-tag bypass" :title="unsupported(n, sig.format, !isBus(sig.id))">⚠ not on {{ sig.format }}</span>
            <span v-if="DYNAMICS.has(n.type)" class="grbar"><span class="gr"><span :style="{ width: `${Math.min(100, ((meters[n.id]?.gain_reduction_db || 0) / 20) * 100)}%` }"></span></span></span>
            <span class="spacer"></span>
            <button class="btn small secondary" :title="`Copy ${n.id} to the clipboard`" :aria-label="`Copy ${n.id}`" @click="copyNode(n)">{{ copied === n.id ? "✓" : "⧉" }}</button>
            <button v-if="latestOf(n.type)" class="btn small secondary" :title="`Paste the copied ${NODE_TYPES[n.type]?.label || n.type} (from ${nameOf(latestOf(n.type).from)}) onto ${n.id}: its parameters`" :aria-label="`Paste onto ${n.id}`" @click="pasteOnto(n)">⇩</button>
            <button class="btn small bypass" :class="{ secondary: !n.bypass }" :aria-pressed="n.bypass" @click="bypass(n)">Bypass</button>
            <button class="btn small secondary" :disabled="i === 0" :aria-label="`Move ${n.id} earlier`" @click="move(n.id, i - 1)">←</button>
            <button class="btn small secondary" :disabled="i === chain.length - 1" :aria-label="`Move ${n.id} later`" @click="move(n.id, i + 1)">→</button>
            <button class="btn small secondary" :aria-label="isOpen(n.id) ? `Collapse ${n.id}` : `Expand ${n.id}`" @click="open[n.id] = !isOpen(n.id)">{{ isOpen(n.id) ? "▾" : "▸" }}</button>
            <ConfirmButton label="Delete" small @confirm="remove(n.id)" />
          </div>
          <div v-if="isOpen(n.id)" class="nbody">
            <NodeEditor :signal="sig.id" :node="n" />
          </div>
        </div>
        <div class="dropzone" :data-drop="chain.length" :class="{ over: drag.active && drag.over === chain.length }">
          {{ chain.length ? "Drop here to append" : "No processing yet: drag a node type here, or click one in the palette." }}
        </div>
      </div>
      <FlowView v-if="sig" :signal="sig" />
    </div>
    <div v-if="drag.active" class="dragghost" :style="{ left: `${drag.x + 12}px`, top: `${drag.y + 8}px` }">{{ drag.label }}</div>
  </div>
</template>

<style scoped>
.clipbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.35rem;
}
.clipchip {
  display: inline-flex;
  gap: 1px;
}
</style>
