<script setup>
// Logic graphs (§11): blocks with typed ports wired together, evaluated at 100 Hz. The canvas (§11.3)
// takes blocks from the palette (drag or click; the palette is the server's block catalog), draws
// cables between ports of types that convert (D158), zooms and pans, groups blocks into macros and
// opens them. Every edit is one atomic batch (POST /api/v1/logic-batch) and one step of undo and redo;
// selected blocks copy and paste with their cables. A refused edit shows the server's issues as red
// markers on the blocks, ports and cables (D162). The list under the canvas edits the same graph or
// macro. Live values, soft keys and the action log (dry run) are polled while the tab is open.
import { computed, nextTick, onMounted, onUnmounted, provide, reactive, ref, watch } from "vue";
import { VueFlow, useVueFlow } from "@vue-flow/core";
import { MiniMap } from "@vue-flow/minimap";
import { Controls } from "@vue-flow/controls";
import "@vue-flow/core/dist/style.css";
import "@vue-flow/minimap/dist/style.css";
import "@vue-flow/controls/dist/style.css";
import ConfirmButton from "./ConfirmButton.vue";
import LogicNode from "./LogicNode.vue";
import LogicParam from "./LogicParam.vue";
import { api, exchange, fmtDb } from "../api.js";
import { act, live, room, signalOf, signals, vcas } from "../store.js";
import { PORT_TYPES, boundary, canWire, clipboard, copyOf, forget, freshId, group, history, historyOf, pasted, portName, portsOf, record, resource, same, sameEdit, ungroup } from "../logicEdit.js";

const { applyNodeChanges, applyEdgeChanges, fitView, screenToFlowCoordinate, getSelectedNodes, getSelectedEdges } = useVueFlow("logic");

const MAX_BLOCKS = 64;
const graphs = computed(() => live.mix?.logic || []);
// The mix leaves out `logic_macros` when there is none.
const macros = computed(() => live.mix?.logic_macros || []);
const snapshots = computed(() => live.mix?.snapshots || []);
const selected = ref("");
const graph = computed(() => graphs.value.find((g) => g.id === selected.value) || graphs.value[0] || null);
const name = ref("");

// ---- the block catalog (GET /api/v1/logic-blocks) ---------------------------------------------

const catalog = ref([]);
const entryOf = (type) => catalog.value.find((e) => e.type === type);
const categoryOf = (type) =>
  String(entryOf(type)?.category || "logic")
    .toLowerCase()
    .replace(/s$/, "");
const typeLabel = (type) => entryOf(type)?.label || type;
const CATEGORIES = [
  ["input", "Inputs"],
  ["logic", "Logic"],
  ["action", "Actions"],
  ["macro", "Macros"],
];
const PEERS = ["master", "s1", "s2", "s3", "s4"];

// ---- what the canvas shows: the graph, or a macro opened from one of its instances -------------

const path = ref([]); // [{ instance, macro }] from the graph down to the open macro
const openMacro = computed(() => {
  const top = path.value.at(-1);
  return top ? macros.value.find((m) => m.id === top.macro) || null : null;
});
const doc = computed(() => (openMacro.value ? { kind: "macro", id: openMacro.value.id } : graph.value ? { kind: "graph", id: graph.value.id } : null));
const view = computed(() => openMacro.value || graph.value);
const prefix = computed(() => path.value.map((p) => `${p.instance}.`).join(""));
const keyOf = (d) => `${d.kind}:${d.id}`;
const urlOf = (d) => (d.kind === "graph" ? `/api/v1/logic/${d.id}` : `/api/v1/logic-macros/${d.id}`);
watch(
  () => graph.value?.id,
  () => (path.value = []),
);
watch(openMacro, (m) => {
  if (!m && path.value.length) path.value = [];
});

// ---- polled: live values, soft keys, actions, ports -------------------------------------------

// Updated in place, so that only what shows a value redraws on each poll: redrawing the whole tab would
// reset a number being typed in the list.
const values = reactive({});
const detail = reactive({ buttons: [], actions: [], ports: {}, warnings: [] });
const fired = {}; // block -> last event count seen
let timer = 0;
let polling = false;

// What another graph or macro showed must not stay: values, ports and actions start empty there.
function forgetShown() {
  for (const k of Object.keys(values)) delete values[k];
  for (const k of Object.keys(fired)) delete fired[k];
  Object.assign(detail, { buttons: [], actions: [], ports: {}, warnings: [] });
}
watch(
  () => (doc.value ? keyOf(doc.value) : ""),
  () => {
    forgetShown();
    // A dialog belongs to the graph or macro it was opened in.
    confirming.value = null;
    choosing.value = null;
    grouping.value = null;
  },
);

// One poll at a time; an answer for a graph or macro that is no longer open is dropped.
async function refresh() {
  if (polling) return;
  polling = true;
  try {
    await poll();
  } finally {
    polling = false;
  }
}
async function poll() {
  const g = graph.value;
  const m = openMacro.value;
  if (!g) return;
  const d = await api.get(`/api/v1/logic/${g.id}`).catch(() => null);
  const md = m ? await api.get(`/api/v1/logic-macros/${m.id}`).catch(() => null) : null;
  if (!d || (m && !md) || g.id !== graph.value?.id || m?.id !== openMacro.value?.id) return;
  const v = d.values || {};
  for (const k of Object.keys(values)) if (!(k in v)) delete values[k];
  Object.assign(values, v);
  // Only real changes: the list is drawn from these, and a redraw would reset a number being typed.
  const changed = (k, v) => JSON.stringify(v) !== JSON.stringify(detail[k]) && (detail[k] = v);
  changed("buttons", d.buttons || []);
  changed("actions", d.actions || []);
  changed("ports", (md || d).ports || {});
  changed("warnings", (md || d).warnings || []);
  showValues();
}

const ports = (b) => detail.ports[b.id] || portsOf(b, catalog.value, macros.value);
const valueOf = (b) => values[prefix.value + b.id];
const typeOf = (b) => {
  const p = ports(b);
  return p.outputs[0]?.type || p.inputs[0]?.type || "";
};

function shown(b) {
  const v = valueOf(b);
  if (v === undefined) return "";
  const t = typeOf(b);
  if (typeof v === "boolean") return v ? "on" : "off";
  if (typeof v === "string") return v.length > 24 ? `“${v.slice(0, 23)}…”` : `“${v}”`;
  if (t === "event") return `×${v}`;
  if (v === null) return t === "level" ? "−∞ dB" : "±∞";
  if (t === "level") return `${fmtDb(v)} dB`;
  if (t === "boolean") return v >= 0.5 ? "on" : "off";
  return String(Number(Number(v).toFixed(2)));
}
function isOn(b) {
  const v = valueOf(b);
  if (typeof v === "boolean") return v;
  return typeOf(b) === "boolean" && typeof v === "number" ? v >= 0.5 : null;
}

const pressed = (b) => detail.buttons.includes(b.id);
function press(b, down) {
  return act(() => api.put(`/api/v1/logic/${graph.value.id}/buttons/${b.id}`, { pressed: down }));
}
function fire(b) {
  return act(() => api.post(`/api/v1/logic/${graph.value.id}/trigger/${b.id}`));
}
const when = (t) => (t > 1e12 ? new Date(t).toLocaleTimeString() : `${t} ms`);

// ---- edits: one batch each, recorded for undo -------------------------------------------------

const issues = ref([]); // the last refused edit's issues (D162)
const rejected = ref(null); // { key, res }: what that edit wanted the open graph or macro to be

let queued = Promise.resolve();
// A failed step must not stop the ones after it.
const failed = (e) => (live.actionError = e.message);

// POST /api/v1/logic-batch: the answer, { conflict } on 409, { refused } on 400 (issues shown), or
// nothing (the request failed: network, 5xx).
async function send(body, want) {
  try {
    const { data: done, revision } = await exchange("/api/v1/logic-batch", { method: "POST", body });
    adopt(body, done, revision);
    live.actionError = "";
    issues.value = [];
    rejected.value = null;
    return done;
  } catch (e) {
    if (e.status === 409) return { conflict: true };
    // A refused edit's issues are shown on the canvas and in the box above it, not in the banner.
    if (e.status === 400 && Array.isArray(e.data?.issues) && e.data.issues.length) {
      issues.value = e.data.issues;
      rejected.value = want || null;
    } else live.actionError = e.message;
    return e.status === 400 ? { refused: true } : null;
  }
}

// A write's answer is the newest state of what it wrote: shown at once, without waiting for the pushed
// mix, which may come a meter tick later (store.js then drops pushes older than this revision).
function adopt(body, done, revision) {
  // Not over a push of the same or a newer revision: it already has this write, and maybe more.
  if (!live.mix || !done || (revision !== null && revision <= live.revision)) return;
  for (const [kind, key] of [
    ["graph", "logic"],
    ["macro", "logic_macros"],
  ]) {
    let list = [...(live.mix[key] || [])];
    for (const r of done[`${kind}s`] || []) {
      const i = list.findIndex((x) => x.id === r.id);
      if (i >= 0) list[i] = resource(r);
      else list.push(resource(r));
    }
    const gone = new Set(body[`delete_${kind}s`] || []);
    list = list.filter((x) => !gone.has(x.id));
    live.mix[key] = list;
  }
  if (revision !== null) live.revision = revision;
}

// Reads `reads` (graphs and macros; with `lists`, also every graph and macro as `cur.graphs` and
// `cur.macros`), lets `build` make
// the new resources, writes them as one batch with the revisions read (a 409 means someone else changed
// them: nothing is written) and, with a label, records the edit in the open document's history.
// `build` returns the batch body, a string (why not), or null.
function edit(label, reads, build, { lists = false } = {}) {
  const d = doc.value;
  if (!d) return queued;
  const key = keyOf(d);
  queued = queued
    .then(async () => {
      const cur = {};
      for (const r of reads) cur[keyOf(r)] = await api.get(urlOf(r)).catch(() => null);
      if (lists) {
        cur.graphs = await api.get("/api/v1/logic").catch(() => null);
        cur.macros = await api.get("/api/v1/logic-macros").catch(() => null);
        if (!cur.graphs || !cur.macros) return void (live.actionError = "The logic could not be read; nothing was written.");
      }
      const made = build(cur);
      if (!made) return;
      if (typeof made === "string") {
        live.actionError = made;
        return;
      }
      const body = { graphs: [], macros: [], delete_graphs: [], delete_macros: [], ...made };
      for (const kind of ["graph", "macro"]) body[`${kind}s`] = body[`${kind}s`].map((r) => ({ ...resource(r), ...(cur[`${kind}:${r.id}`]?.revision ? { revision: cur[`${kind}:${r.id}`].revision } : {}) }));
      const mine = body[`${d.kind}s`].find((r) => r.id === d.id);
      const done = await send(body, mine && { key, res: mine });
      if (done?.conflict) live.actionError = "Changed elsewhere meanwhile; nothing was written. Try again.";
      if (!done || done.conflict || done.refused) return;
      const changes = [];
      for (const kind of ["graph", "macro"]) {
        for (const r of body[`${kind}s`]) {
          const before = cur[`${kind}:${r.id}`];
          const after = (done[`${kind}s`] || []).find((x) => x.id === r.id);
          if (!before || !same(before, after)) changes.push({ kind, id: r.id, before: resource(before), after: resource(after) });
        }
        for (const id of body[`delete_${kind}s`]) changes.push({ kind, id, before: resource(cur[`${kind}:${id}`]), after: null });
      }
      if (changes.length && label) record(key, { label, changes });
    })
    .catch(failed);
  return queued;
}

// An edit of the open graph or macro only: `mutate` gets it and returns the fields to change.
function commit(label, mutate) {
  const d = doc.value;
  return edit(label, d ? [d] : [], (cur) => {
    const c = cur[keyOf(d)];
    const fields = c && mutate(c);
    return fields ? { [`${d.kind}s`]: [{ ...resource(c), ...fields }] } : null;
  });
}
const patch = (label, body) => commit(label, () => body);

// "On" and "dry run" of graphs: the operator's switches, written at once and never undone or redone (an
// undo must not take a graph live).
// Resolves to whether every graph was written.
function setGraphs(ids, fields) {
  let ok = false;
  queued = queued
    .then(async () => {
      const body = { graphs: [] };
      for (const id of ids) {
        const c = await api.get(`/api/v1/logic/${id}`).catch(() => null);
        if (!c) return void (live.actionError = `Graph ${id} could not be read; nothing was written.`);
        body.graphs.push({ ...resource(c), ...fields, revision: c.revision });
      }
      const done = await send(body);
      if (done?.conflict) live.actionError = "Changed elsewhere meanwhile; nothing was written. Try again.";
      ok = !!done && !done.conflict && !done.refused;
    })
    .catch(failed);
  return queued.then(() => ok);
}
// A switch in the settings row; it flips back when the write fails.
async function toggle(e, field) {
  const box = e.target;
  if (!(await setGraphs([graph.value.id], { [field]: box.checked }))) box.checked = !box.checked;
}

// Undo (back) or redo. Each graph and macro of the entry is read again and must still be what this client
// left (else the history is cleared: someone else changed it); it is written back with the revision just
// read, keeping its "on" and "dry run". A refused step (400) is dropped from the history; a failed one
// (network, 5xx) stays. In a live graph, a step that re-makes actions or blocks that remember asks first.
function step(back) {
  const d = doc.value;
  if (!d) return queued;
  const key = keyOf(d);
  // What the step would re-make, against the graphs and macros as they are now (read, not the last push).
  queued = queued
    .then(async () => {
      const entry = history[key]?.[back ? "undo" : "redo"].at(-1);
      if (!entry) return;
      const remade = [];
      for (const c of entry.changes) {
        const had = (await api.get(urlOf(c)).catch(() => null))?.blocks || [];
        const want = back ? c.before : c.after;
        remade.push(...(want?.blocks || []).filter((b) => !had.some((x) => x.id === b.id && same({ blocks: [x] }, { blocks: [b] }))));
      }
      guardLive(remade, `${back ? "Undo" : "Redo"} of "${entry.label}" re-makes blocks: actions among them act at once, and blocks that remember start empty.`, () => stepNow(back, d));
    })
    .catch(failed);
  return queued;
}
function stepNow(back, d) {
  const key = keyOf(d);
  const cleared = () => {
    forget(key);
    live.notice = "Changed elsewhere meanwhile: the undo history of this graph is cleared.";
  };
  queued = queued
    .then(async () => {
      const h = historyOf(key);
      const from = back ? h.undo : h.redo;
      const entry = from.at(-1);
      if (!entry) return;
      const body = { graphs: [], macros: [], delete_graphs: [], delete_macros: [] };
      for (const c of entry.changes) {
        let cur;
        try {
          cur = await api.get(urlOf(c));
        } catch (e) {
          if (e.status !== 404) return void (live.actionError = e.message);
          cur = null;
        }
        const expect = back ? c.after : c.before;
        const want = back ? c.before : c.after;
        if (expect ? !cur || !sameEdit(cur, expect) : cur) return cleared();
        if (!want) body[`delete_${c.kind}s`].push(c.id);
        else body[`${c.kind}s`].push(cur ? { ...want, ...(c.kind === "graph" ? { enabled: cur.enabled, dry_run: cur.dry_run } : {}), revision: cur.revision } : { ...want, ...(c.kind === "graph" ? { dry_run: true } : {}) });
      }
      const done = await send(body);
      if (done?.conflict) return cleared();
      if (!done) return;
      from.pop();
      if (done.refused) return;
      (back ? h.redo : h.undo).push(entry);
      live.notice = `${back ? "Undone" : "Redone"}: ${entry.label}.`;
    })
    .catch(failed);
  return queued;
}
const undo = () => step(true);
const redo = () => step(false);
const undoNext = computed(() => (doc.value ? history[keyOf(doc.value)]?.undo.at(-1)?.label || "" : ""));
const redoNext = computed(() => (doc.value ? history[keyOf(doc.value)]?.redo.at(-1)?.label || "" : ""));

async function addGraph() {
  // A new graph starts in dry run: its actions are listed, not done, until the operator goes live.
  const body = { dry_run: true };
  if (name.value.trim()) body.name = name.value.trim();
  const made = await act(() => api.post("/api/v1/logic", body), "Logic graph added (dry run).");
  if (made) {
    name.value = "";
    selected.value = made.id;
  }
}

// Histories of graphs and macros that are gone (deleted here or elsewhere) are dropped.
watch(
  () => (live.mix ? [...graphs.value.map((g) => `graph:${g.id}`), ...macros.value.map((m) => `macro:${m.id}`)].join(",") : null),
  (now) => {
    if (now === null) return;
    const there = new Set(now.split(","));
    for (const k of Object.keys(history)) if (!there.has(k)) forget(k);
  },
);

async function removeGraph(id) {
  if (await act(() => api.del(`/api/v1/logic/${id}`))) forget(`graph:${id}`);
}

// ---- blocks and their parameters --------------------------------------------------------------

const addType = ref("const");
const addSignal = ref("");

// The choices of a parameter, [{ id, name }], or null for a typed value.
function optionsOf(p, b) {
  if (p.options) return p.options.map((o) => (typeof o === "object" ? { id: o.id ?? o.value, name: o.name ?? o.label ?? o.id ?? o.value } : { id: o, name: o }));
  const named = (list) => list.map((x) => ({ id: x.id, name: x.name || x.id }));
  if (p.kind === "signal") return named(signals.value);
  if (p.kind === "signal-or-vca") return named([...signals.value, ...vcas.value]);
  if (p.kind === "vca") return named(vcas.value);
  if (p.kind === "snapshot") return named(snapshots.value);
  if (p.kind === "monitor") return named(room.value.monitors || []);
  if (p.kind === "mic") return named(room.value.talkback?.mics || []);
  if (p.kind === "destination") return named(room.value.talkback?.destinations || []);
  if (p.kind === "node") return (signalOf(b?.signal)?.chain || []).map((n) => ({ id: n.id, name: `${n.id} (${n.type})` }));
  if (p.kind === "macro") return named(macros.value);
  if (p.kind === "peer") return PEERS.map((x) => ({ id: x, name: x }));
  if (p.kind === "port-type") return PORT_TYPES.filter((t) => t !== "event" || b?.type === "macro-in" || b?.type === "macro-out").map((t) => ({ id: t, name: t }));
  return null;
}
const JSON_KINDS = ["points", "source"];
const NUMBER_KINDS = ["number", "integer"];

// The parameters a new block starts with. A target (the strip, VCA, snapshot, monitor, mic, destination,
// node or parameter it reads or acts on) is never guessed: it stays "" (the block is valid and inert,
// with a warning) until the operator chooses one, unless a strip or bus is picked in the add row. Other
// parameters take the catalog's default, else 1000 ms or the lowest number allowed. An action's other
// parameters without a default (a webhook's URL, a TSL index) are `needs`, asked for before the block is
// made. A string is why the block cannot be made.
const BLANK_KINDS = ["signal", "signal-or-vca", "vca", "snapshot", "monitor", "mic", "destination", "node", "param"];
function paramsFor(type, inView) {
  const e = entryOf(type);
  const action = categoryOf(type) === "action";
  const params = {};
  const needs = [];
  for (const p of e?.params || []) {
    let v = p.default ?? (p.kind === "source" ? null : undefined);
    if (p.name === "name" && (type === "macro-in" || type === "macro-out")) v = freshId(type === "macro-in" ? "in" : "out", new Set(inView.filter((b) => b.type === type).map((b) => b.name)));
    if (v === undefined && BLANK_KINDS.includes(p.kind)) v = p.kind.startsWith("signal") && signals.value.some((x) => x.id === addSignal.value) ? addSignal.value : "";
    if (v === undefined && action) {
      needs.push(p);
      continue;
    }
    if (v === undefined && p.name === "ms") v = 1000;
    if (v === undefined) {
      const opts = optionsOf(p, { type, ...params });
      if (opts) v = opts[0]?.id;
      else if (NUMBER_KINDS.includes(p.kind)) v = Math.max(p.min ?? 0, Math.min(p.max ?? 0, 0));
      else if (p.kind === "boolean") v = false;
      else if (p.kind === "points") v = [
          [0, 0],
          [1, 1],
        ];
      else v = "";
    }
    if (v === undefined) return `${typeLabel(type)} needs a ${p.kind}, and there is none yet.`;
    params[p.name] = v;
  }
  return { params, needs };
}

// A new block at `at` (canvas coordinates), or in a row; it is selected. `extra` sets parameters. An
// action whose target is not chosen yet asks for it first.
const choosing = ref(null); // { type, at, extra, draft, needs } while an action's target is asked for
function place(type, at, extra = {}) {
  const v = view.value;
  if (!v) return;
  const r = paramsFor(type, v.blocks);
  if (typeof r === "string") return void (live.actionError = r);
  const needs = r.needs.filter((p) => !(p.name in extra));
  if (needs.length) choosing.value = { type, at, extra, needs, draft: { id: "new", type, ...r.params, ...extra } };
  // A new action has no wired input, so it does nothing yet; a macro instance's inner actions may act.
  else if (type === "macro") guardLive([{ type }], "A macro's actions act at once, with whatever its inputs read.", () => add(type, at, { ...r.params, ...extra }));
  else add(type, at, { ...r.params, ...extra });
}
const chosen = computed(() => choosing.value?.needs.every((p) => choosing.value.draft[p.name] !== undefined && choosing.value.draft[p.name] !== ""));
function choose(p, raw) {
  const n = Number(raw);
  choosing.value.draft[p.name] = NUMBER_KINDS.includes(p.kind) ? (String(raw).trim() === "" || !Number.isFinite(n) ? undefined : n) : raw;
}
function addChosen() {
  const c = choosing.value;
  if (!c || !chosen.value) return;
  choosing.value = null;
  const { id: _id, type: _type, ...params } = c.draft;
  add(c.type, c.at, params);
}
function add(type, at, params) {
  commit(`Add ${typeLabel(type)}`, (g) => {
    if (g.blocks.length >= MAX_BLOCKS) return void (live.actionError = `A graph or macro holds at most ${MAX_BLOCKS} blocks.`);
    const id = freshId(type, new Set(g.blocks.map((b) => b.id)));
    const pos = at || { x: 40 + (g.blocks.length % 5) * 220, y: 80 + Math.floor(g.blocks.length / 5) * 140 };
    want = new Set([id]);
    return { blocks: [...g.blocks, { id, type, x: Math.round(pos.x), y: Math.round(pos.y), ...params }] };
  });
}

function removeBlock(id) {
  commit(`Delete ${id}`, (g) => ({
    blocks: g.blocks.filter((b) => b.id !== id),
    wires: g.wires.filter((w) => w.from_block !== id && w.to_block !== id),
  }));
}

function setParam(b, p, raw) {
  let value = raw;
  if (NUMBER_KINDS.includes(p.kind)) {
    value = Number(raw);
    if (!Number.isFinite(value) || (p.kind === "integer" && !Number.isInteger(value))) return void (live.actionError = `${b.id}: ${p.name} must be ${p.kind === "integer" ? "a whole number" : "a number"}.`);
  } else if (JSON_KINDS.includes(p.kind)) {
    try {
      value = JSON.parse(raw);
    } catch {
      return void (live.actionError = `${b.id}: ${p.name} is not valid JSON.`);
    }
  }
  commit(`Set ${b.id} ${p.name}`, (g) => ({ blocks: g.blocks.map((x) => (x.id === b.id ? { ...x, [p.name]: value } : x)) }));
}

const paramsOf = (b) => (entryOf(b.type)?.params || []).filter((p) => p.name !== "x" && p.name !== "y");

function labelOf(b) {
  const label = typeLabel(b.type);
  if (b.type === "macro") return `${macros.value.find((m) => m.id === b.macro)?.name || b.macro}`;
  if (b.type === "schedule") return `${label}: ${hhmm(b.start_minute)}–${hhmm(b.end_minute)}`;
  if (paramsOf(b).some((q) => BLANK_KINDS.includes(q.kind) && b[q.name] === "")) return `${label}: no target yet`;
  const p = paramsOf(b).find((q) => q.kind !== "port-type" && b[q.name] !== undefined && b[q.name] !== null && typeof b[q.name] !== "object" && b[q.name] !== "");
  if (!p) return label;
  const v = b[p.name];
  const opt = optionsOf(p, b)?.find((o) => o.id === v);
  const unit = /_ms$|^ms$/.test(p.name) ? " ms" : /_db$/.test(p.name) ? " dB" : "";
  return `${label}: ${opt?.name ?? v}${unit}`;
}
const hhmm = (m) => `${String(Math.floor((m || 0) / 60)).padStart(2, "0")}:${String((m || 0) % 60).padStart(2, "0")}`;

// ---- cables -----------------------------------------------------------------------------------

const wireKey = (w) => `${w.from_block}.${w.from_port}>${w.to_block}.${w.to_port}`;
const wire = ref({ from: "", fromPort: "", to: "", port: "" });
const blockOf = (id) => view.value?.blocks.find((b) => b.id === id);
const outsOf = (id) => (blockOf(id) ? ports(blockOf(id)).outputs : []);
const insOf = (id) => (blockOf(id) ? ports(blockOf(id)).inputs : []);
const portType = (id, side, name) => (side === "out" ? outsOf(id) : insOf(id)).find((p) => p.name === name)?.type;

function connect(from, fromPort, to, port) {
  commit("Connect", (g) => {
    const wires = g.wires.filter((w) => !(w.to_block === to && w.to_port === port));
    return { wires: [...wires, { from_block: from, from_port: fromPort, to_block: to, to_port: port }] };
  });
}

function addWire() {
  const { from, fromPort, to, port } = wire.value;
  if (!from || !fromPort || !to || !port) return;
  connect(from, fromPort, to, port);
}

function removeWire(w) {
  commit("Delete cable", (g) => ({ wires: g.wires.filter((x) => wireKey(x) !== wireKey(w)) }));
}

// ---- live or not --------------------------------------------------------------------------------

// The graphs that run for real (on, not in dry run) and contain the open document: the graph itself, or
// every live graph that uses the open macro, also through other macros.
function uses(blocks, id, seen = new Set()) {
  return blocks.some((b) => b.type === "macro" && (b.macro === id || (!seen.has(b.macro) && seen.add(b.macro) && uses(macros.value.find((m) => m.id === b.macro)?.blocks || [], id, seen))));
}
const liveGraphs = computed(() => {
  const running = graphs.value.filter((g) => g.enabled && !g.dry_run);
  if (!openMacro.value) return running.filter((g) => g.id === graph.value?.id);
  return running.filter((g) => uses(g.blocks, openMacro.value.id));
});
const state = computed(() => (!graph.value ? "" : openMacro.value ? (liveGraphs.value.length ? "live" : "off") : !graph.value.enabled ? "off" : graph.value.dry_run ? "dry" : "live"));

// Group, ungroup and paste re-make blocks: in a live graph, actions among them apply at once and blocks
// that remember start empty (D172). Then the operator confirms, or goes to dry run first.
const confirming = ref(null); // { text, run }
const risky = (blocks) => blocks.some((b) => categoryOf(b.type) === "action" || entryOf(b.type)?.remembers || b.type === "macro");
function guardLive(blocks, text, run) {
  if (liveGraphs.value.length && risky(blocks)) confirming.value = { text, run };
  else run();
}
// "Switch to dry run first" goes ahead only when the switch was written.
async function confirmLive(dryFirst) {
  const c = confirming.value;
  confirming.value = null;
  if (!c) return;
  if (dryFirst && !(await setGraphs(liveGraphs.value.map((g) => g.id), { dry_run: true }))) {
    live.actionError = `Not switched to dry run, so nothing was changed. ${live.actionError || ""}`.trim();
    return;
  }
  c.run();
}

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
const selectedInstance = computed(() => (selectedIds.value.length === 1 && blockOf(selectedIds.value[0])?.type === "macro" ? blockOf(selectedIds.value[0]) : null));
function deleteSelection() {
  for (const id of selectedIds.value) doom("blocks", id);
  for (const e of getSelectedEdges.value) if (!e.data?.ghost) doom("wires", e.id);
}

// Copies from the graph or macro as the server has it now (read in the queue, after this client's own
// edits), not from the last pushed mix.
let copying = 0;
function copy() {
  const d = doc.value;
  const ids = selectedIds.value;
  if (!d || !ids.length) return false;
  copying++;
  queued = queued
    .then(async () => {
      const cur = await api.get(urlOf(d)).catch(() => null);
      if (!cur) return void (live.actionError = "The graph could not be read; nothing was copied.");
      Object.assign(clipboard, copyOf(cur, ids), { pastes: 0 });
      live.notice = `${ids.length} block${ids.length > 1 ? "s" : ""} copied.`;
    })
    .catch(failed)
    .finally(() => copying--);
  return true;
}

// The clipboard into the open graph or macro (any one), with new ids, a bit further down and right on
// each paste. A macro's own inputs and outputs are left out when pasting into a graph.
function paste() {
  if (!view.value || (!clipboard.blocks.length && !copying)) return false;
  // After a copy that is still reading.
  queued = queued.then(pasteClip).catch(failed);
  return true;
}
function pasteClip() {
  if (!view.value || !clipboard.blocks.length) return;
  const inGraph = !openMacro.value;
  const own = (t) => inGraph && (t === "macro-in" || t === "macro-out");
  const drop = new Set(clipboard.blocks.filter((b) => own(b.type)).map((b) => b.id));
  const clip = { blocks: clipboard.blocks.filter((b) => !drop.has(b.id)), wires: clipboard.wires.filter((w) => !drop.has(w.from_block) && !drop.has(w.to_block)) };
  if (!clip.blocks.length) return;
  guardLive(clip.blocks, "Pasted actions act at once in a live graph, and pasted blocks that remember start empty.", () => pasteNow(clip));
}
function pasteNow(clip) {
  const offset = 40 * ++clipboard.pastes;
  commit(`Paste ${clip.blocks.length} block${clip.blocks.length > 1 ? "s" : ""}`, (g) => {
    if (g.blocks.length + clip.blocks.length > MAX_BLOCKS) return void (live.actionError = `Pasting gives ${g.blocks.length + clip.blocks.length} blocks; a graph or macro holds at most ${MAX_BLOCKS}.`);
    const p = pasted(g, clip, offset);
    want = new Set(p.ids);
    return { blocks: [...g.blocks, ...p.blocks], wires: [...g.wires, ...p.wires] };
  });
  return true;
}

// Whether the last click was on the canvas (then Ctrl+C is for blocks, whatever text is selected).
let canvasLast = false;
const onDown = (e) => (canvasLast = !!e.target?.closest?.(".logiccanvas"));

// Ctrl/Cmd + Z, Shift+Z (or Y), C, V while no text field has the focus.
function onKey(e) {
  if (!view.value || !(e.ctrlKey || e.metaKey) || e.altKey) return;
  if (e.target?.closest?.("input, textarea, select, [contenteditable='true']")) return;
  const k = e.key.toLowerCase();
  let done = true;
  if (k === "z" && !e.shiftKey) undo();
  else if ((k === "z" && e.shiftKey) || k === "y") redo();
  // Ctrl+C copies the text selected on the page, unless the canvas was clicked last.
  else if (k === "c") done = (canvasLast || !window.getSelection()?.toString()) && copy();
  else if (k === "v") done = paste();
  else done = false;
  if (done) e.preventDefault();
}

// ---- macros (D163) ----------------------------------------------------------------------------

const grouping = ref(null); // { ids, name, inputs, outputs } while the group dialog is open

function startGroup() {
  const v = view.value;
  const ids = selectedIds.value;
  if (!v || !ids.length) return;
  if (ids.some((id) => ["macro-in", "macro-out"].includes(blockOf(id)?.type))) return void (live.actionError = "A macro's own inputs and outputs cannot be grouped.");
  const b = boundary(v, ids, ports);
  const size = ids.length + b.inputs.filter((p) => p.crossing).length + b.outputs.filter((p) => p.crossing).length;
  if (size > MAX_BLOCKS) return void (live.actionError = `The macro would hold ${size} blocks with its inputs and outputs; a macro holds at most ${MAX_BLOCKS}.`);
  grouping.value = { ids, name: "", ...b };
}
// The ports a group exposes, as a comparable key.
const portsKey = (b) => JSON.stringify([b.inputs, b.outputs].map((l) => l.map((p) => [p.block, p.port, p.type, p.crossing])));

const groupProblem = computed(() => {
  const gp = grouping.value;
  if (!gp) return "";
  for (const side of ["inputs", "outputs"]) {
    const names = gp[side].filter((p) => p.expose).map((p) => portName(p.name));
    if (new Set(names).size !== names.length) return `Two exposed ${side} have the same name.`;
  }
  const size = gp.ids.length + gp.inputs.filter((p) => p.expose).length + gp.outputs.filter((p) => p.expose).length;
  if (size > MAX_BLOCKS) return `The macro would hold ${size} blocks; at most ${MAX_BLOCKS}.`;
  return "";
});

function makeGroup() {
  const gp = grouping.value;
  const d = doc.value;
  if (!gp || !d || groupProblem.value) return;
  grouping.value = null;
  const members = gp.ids.map(blockOf).filter(Boolean);
  guardLive(members, "Grouping starts the blocks that remember (latches, toggles, counters, timers) empty, and actions apply their input again at once.", () => groupNow(gp, d));
}
function groupNow(gp, d) {
  const exposed = { inputs: gp.inputs.map((p) => ({ ...p, name: portName(p.name) })), outputs: gp.outputs.map((p) => ({ ...p, name: portName(p.name) })) };
  edit(
    "Group into macro",
    [d],
    (cur) => {
      const c = cur[keyOf(d)];
      if (!c) return null;
      // The ports shown in the dialog must still be the group's ports (someone may have rewired it).
      if (gp.ids.some((id) => !c.blocks.some((b) => b.id === id)) || portsKey(boundary(c, gp.ids, ports)) !== portsKey(gp)) return "The blocks changed meanwhile; group them again.";
      const macroId = freshId("macro", new Set(cur.macros.map((m) => m.id)));
      const instanceId = freshId("macro", new Set(c.blocks.map((b) => b.id)));
      const r = group(c, gp.ids, exposed, { macroId, name: gp.name.trim() || macroId, instanceId });
      want = new Set([instanceId]);
      const body = { graphs: [], macros: [r.macro] };
      body[`${d.kind}s`].push({ ...resource(c), blocks: r.blocks, wires: r.wires });
      return body;
    },
    { lists: true },
  );
}

function ungroupSelected() {
  const inst = selectedInstance.value;
  const d = doc.value;
  if (!inst || !d) return;
  const inner = macros.value.find((x) => x.id === inst.macro)?.blocks || [];
  guardLive([inst, ...inner], "Ungrouping starts the blocks that remember empty, and actions apply their input again at once.", () => ungroupNow(inst, d));
}
function ungroupNow(inst, d) {
  const m = { kind: "macro", id: inst.macro };
  edit("Ungroup", [d, m], (cur) => {
    const c = cur[keyOf(d)];
    const def = cur[keyOf(m)];
    if (!c || !def) return `Macro ${inst.macro} does not exist.`;
    const r = ungroup(c, inst.id, def);
    if (r.blocks.length > MAX_BLOCKS) return `Ungrouping gives ${r.blocks.length} blocks; a graph or macro holds at most ${MAX_BLOCKS}.`;
    want = new Set(r.ids);
    const body = { graphs: [], macros: [] };
    body[`${d.kind}s`].push({ ...resource(c), blocks: r.blocks, wires: r.wires });
    // The macro goes too when nothing else uses it (read just now).
    const uses = (blocks) => blocks.some((b) => b.type === "macro" && b.macro === def.id);
    const others = [...cur.graphs.filter((g) => !(d.kind === "graph" && g.id === d.id)), ...cur.macros.filter((x) => x.id !== def.id && !(d.kind === "macro" && x.id === d.id))];
    if (!uses(r.blocks) && !others.some((x) => uses(x.blocks))) body.delete_macros = [def.id];
    return body;
  }, { lists: true });
}

function openInstance(b) {
  if (b?.type !== "macro") return;
  path.value = [...path.value, { instance: b.id, macro: b.macro }];
}

// ---- palette ----------------------------------------------------------------------------------

const palette = computed(() => {
  const inMacro = !!openMacro.value;
  const groups = CATEGORIES.map(([id, caption]) => ({
    id,
    caption,
    items: catalog.value.filter((e) => categoryOf(e.type) === id && e.type !== "macro" && (inMacro || (e.type !== "macro-in" && e.type !== "macro-out"))).map((e) => ({ key: e.type, type: e.type, label: e.label || e.type })),
  }));
  const own = groups.find((g) => g.id === "macro");
  // Each macro is a palette item: it adds an instance.
  own.items.push(...macros.value.filter((m) => m.id !== openMacro.value?.id).map((m) => ({ key: `macro:${m.id}`, type: "macro", label: m.name || m.id, extra: { macro: m.id } })));
  return groups.filter((g) => g.items.length);
});

const DRAG = "application/x-mixer-logic-block";
const canvas = ref(null);
function onPaletteDrag(e, item) {
  e.dataTransfer.setData(DRAG, item.key);
  e.dataTransfer.effectAllowed = "copy";
}
function onDragOver(e) {
  if (!e.dataTransfer?.types.includes(DRAG)) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = "copy";
}
const itemOf = (key) => palette.value.flatMap((g) => g.items).find((i) => i.key === key);
function onDrop(e) {
  const item = itemOf(e.dataTransfer?.getData(DRAG));
  if (!item) return;
  e.preventDefault();
  const at = screenToFlowCoordinate({ x: e.clientX, y: e.clientY });
  place(item.type, { x: at.x - 70, y: at.y - 20 }, item.extra);
}
// A click (or a touch, where dragging does not work) puts the block in the middle of the view.
function addFromPalette(item) {
  const r = canvas.value?.getBoundingClientRect();
  if (!r) return place(item.type, undefined, item.extra);
  const n = (view.value?.blocks.length || 0) % 6;
  const at = screenToFlowCoordinate({ x: r.left + r.width / 2 + n * 16, y: r.top + r.height / 2 + n * 16 });
  place(item.type, { x: at.x - 70, y: at.y - 20 }, item.extra);
}

// ---- canvas -----------------------------------------------------------------------------------

const nodes = ref([]);
const edges = ref([]);
let want = null; // block ids to select once they are drawn (a paste, a new block)
// Fit shows the whole graph, but a few blocks are not blown up past 125 %.
const FIT = { padding: 0.2, maxZoom: 1.25 };
const fit = () => fitView(FIT);
const drawing = ref(null);
provide("logicDrawing", drawing);

// The open document's issues: on a block (and port), or on a cable by its index in the refused wires.
const here = computed(() => {
  const d = doc.value;
  return d ? issues.value.filter((i) => (d.kind === "graph" ? i.graph === d.id && !i.macro : i.macro === d.id)) : [];
});
const where = (i) => [i.macro ? `macro ${i.macro}` : i.graph ? `graph ${i.graph}` : "", i.block, i.port].filter(Boolean).join(" · ");

function nodeData(b) {
  const p = ports(b);
  const mine = here.value.filter((i) => i.block === b.id);
  const v = valueOf(b);
  const event = typeOf(b) === "event" && typeof v === "number";
  const fresh = event && fired[b.id] !== undefined && v > fired[b.id];
  if (event) fired[b.id] = v;
  return {
    id: b.id,
    label: labelOf(b),
    note: b.id,
    kind: b.type === "macro" ? "macro" : categoryOf(b.type),
    instance: b.type === "macro",
    inputs: p.inputs,
    outputs: p.outputs,
    live: shown(b),
    on: isOn(b),
    fired: fresh,
    issues: mine.map((i) => i.message),
    badPorts: Object.fromEntries(mine.filter((i) => i.port).map((i) => [i.port, i.message])),
    warnings: detail.warnings.filter((w) => w.block === b.id).map((w) => w.message),
  };
}

// The list's value cell: a component of its own, so that a new value redraws only it.
const LiveText = { props: { block: { type: Object, required: true } }, setup: (p) => () => shown(p.block) };

// Only blocks whose drawing changed are redrawn.
function showValues() {
  for (const n of nodes.value) {
    const b = blockOf(n.id);
    if (!b) continue;
    const data = nodeData(b);
    if (JSON.stringify(data) !== JSON.stringify(n.data)) n.data = data;
  }
}

function edgeOf(w, cls = "") {
  const t = portType(w.from_block, "out", w.from_port) || "number";
  return { id: wireKey(w), source: w.from_block, target: w.to_block, sourceHandle: w.from_port, targetHandle: w.to_port, class: `typed t-${t} ${cls}`, data: w };
}

function rebuild() {
  const v = view.value;
  if (!v) {
    nodes.value = [];
    edges.value = [];
    return;
  }
  let keep = new Set(nodes.value.filter((n) => n.selected).map((n) => n.id));
  if (want && v.blocks.some((b) => want.has(b.id))) {
    keep = want;
    want = null;
  }
  // Cables whose blocks go (another graph or macro opened) go at once; the new ones follow the new blocks.
  const ids = new Set(v.blocks.map((b) => b.id));
  edges.value = edges.value.filter((e) => ids.has(e.source) && ids.has(e.target));
  // The same blocks in the same places: new data only. Replacing the nodes would let a cable drawn in
  // the same tick (a refused one) look for blocks that are being swapped.
  const now = nodes.value;
  const same = now.length === v.blocks.length && v.blocks.every((b, i) => now[i].id === b.id && now[i].position.x === (b.x || 0) && now[i].position.y === (b.y || 0));
  if (same) for (const [i, b] of v.blocks.entries()) Object.assign(now[i], { selected: keep.has(b.id), data: nodeData(b) });
  else nodes.value = v.blocks.map((b) => ({ id: b.id, type: "logic", position: { x: b.x || 0, y: b.y || 0 }, selected: keep.has(b.id), data: nodeData(b) }));
  // A refused edit's cables stay drawn, dashed, until the next edit; those with an issue are red.
  const bad = new Set(here.value.filter((i) => Number.isInteger(i.wire)).map((i) => i.wire));
  const tried = rejected.value?.key === keyOf(doc.value) ? rejected.value.res.wires || [] : [];
  const badKeys = new Set(tried.filter((_, i) => bad.has(i)).map(wireKey));
  const have = new Set(v.wires.map(wireKey));
  const drawn = [
    ...v.wires.map((w) => edgeOf(w, badKeys.has(wireKey(w)) ? "bad" : "")),
    ...tried.filter((w) => !have.has(wireKey(w)) && ids.has(w.from_block) && ids.has(w.to_block)).map((w) => ({ ...edgeOf(w, `ghost${badKeys.has(wireKey(w)) ? " bad" : ""}`), selectable: false, data: { ...w, ghost: true } })),
  ];
  nextTick(() => (edges.value = drawn));
}

// Redrawn when the open graph or macro (or a macro it uses) changes, not on every other mix change; the
// rest is compared by reference.
const drawnKey = computed(() => {
  const v = view.value;
  if (!v) return "";
  const used = new Set(v.blocks.filter((b) => b.type === "macro").map((b) => b.macro));
  return JSON.stringify([keyOf(doc.value), v.blocks, v.wires, macros.value.filter((m) => used.has(m.id))]);
});
watch([drawnKey, () => detail.ports, () => detail.warnings, () => catalog.value, issues, rejected], rebuild, { immediate: true });
// Blocks that wait for their target (D164: valid, never acting).
const waiting = computed(() => new Set(detail.warnings.map((w) => w.block)));
// Another graph or macro: start with all of it in view.
watch(
  () => (doc.value ? keyOf(doc.value) : ""),
  () => setTimeout(fit, 50),
);

function dismissIssues() {
  issues.value = [];
  rejected.value = null;
}

function onNodesChange(changes) {
  applyNodeChanges(changes.filter((c) => c.type !== "remove"));
  for (const c of changes) if (c.type === "remove") doom("blocks", c.id);
}
function onEdgesChange(changes) {
  applyEdgeChanges(changes.filter((c) => c.type !== "remove"));
  for (const c of changes) if (c.type === "remove" && !edges.value.find((e) => e.id === c.id)?.data?.ghost) doom("wires", c.id);
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
// D158: only an output into an input of a type it converts to; never a block into itself. Vue Flow
// also asks for the cables it is given (they have an id): those are drawn as they are, a refused one too.
function isValidConnection(c) {
  if (c.id) return true;
  if (c.source === c.target) return false;
  const from = portType(c.source, "out", c.sourceHandle);
  const to = portType(c.target, "in", c.targetHandle);
  return !!from && !!to && canWire(from, to);
}
function onConnect(c) {
  if (isValidConnection(c)) connect(c.source, c.sourceHandle, c.target, c.targetHandle);
}
function onConnectStart({ nodeId, handleId, handleType }) {
  const type = portType(nodeId, handleType === "source" ? "out" : "in", handleId);
  drawing.value = type ? { type, from: handleType } : null;
}

onMounted(async () => {
  window.addEventListener("keydown", onKey);
  window.addEventListener("pointerdown", onDown, true);
  refresh();
  timer = setInterval(refresh, 250);
  const c = await api.get("/api/v1/logic-blocks").catch(() => null);
  catalog.value = Array.isArray(c) ? c : c?.blocks || [];
});
onUnmounted(() => {
  clearInterval(timer);
  window.removeEventListener("keydown", onKey);
  window.removeEventListener("pointerdown", onDown, true);
});
</script>

<template>
  <div class="panel">
    <h3>Logic</h3>
    <p class="muted">Blocks run 100 times a second. A cable joins an output to an input; the input converts what it gets, but a text feeds only text and an event never feeds text. A loop needs a block that remembers in it (edge, latch, toggle, counter, timer, delay, debounce). In dry run the actions are listed below instead of done.</p>
    <div class="row">
      <select :value="graph?.id || ''" aria-label="Logic graph" @change="selected = $event.target.value">
        <option value="">graph…</option>
        <option v-for="g in graphs" :key="g.id" :value="g.id">{{ g.name }}</option>
      </select>
      <input v-model="name" placeholder="name" aria-label="New logic graph name" />
      <button class="btn small" @click="addGraph">+ Graph</button>
    </div>

    <template v-if="graph">
      <div v-if="!openMacro" class="row" style="margin-top: 0.6rem">
        <input :value="graph.name" aria-label="Logic name" @change="patch('Rename', { name: $event.target.value })" />
        <label class="check"><input type="checkbox" :checked="graph.enabled" @change="toggle($event, 'enabled')" /> on</label>
        <label class="check" title="values are computed, actions are listed below and not done"><input type="checkbox" :checked="graph.dry_run" @change="toggle($event, 'dry_run')" /> dry run</label>
        <ConfirmButton label="Remove" small @confirm="removeGraph(graph.id)" />
      </div>

      <div class="lcrumbs" role="navigation" aria-label="Macro path">
        <button class="btn small secondary" :aria-current="!openMacro" @click="path = []">{{ graph.name }}</button>
        <template v-for="(p, i) in path" :key="i">
          <span class="muted">›</span>
          <button class="btn small secondary" :aria-current="i === path.length - 1" :title="`instance ${p.instance}`" @click="path = path.slice(0, i + 1)">{{ macros.find((m) => m.id === p.macro)?.name || p.macro }}</button>
        </template>
        <template v-if="openMacro">
          <input :value="openMacro.name" aria-label="Macro name" style="width: 12rem" @change="patch('Rename macro', { name: $event.target.value })" />
          <span class="muted small">Editing this macro changes every instance of it.</span>
        </template>
      </div>

      <div class="row tight ltools">
        <span class="lstate" :class="state" data-state :title="state === 'live' ? 'Actions are done' : state === 'dry' ? 'Actions are listed, not done' : 'The graph does not run'">
          {{ state === "live" ? (openMacro ? `LIVE in ${liveGraphs.length} graph${liveGraphs.length > 1 ? "s" : ""}` : "LIVE") : state === "dry" ? "DRY RUN" : openMacro ? "NOT USED LIVE" : "OFF" }}
        </span>
        <span v-if="waiting.size" class="lwait small" data-waiting>{{ waiting.size }} block{{ waiting.size > 1 ? "s" : "" }} without a target (inert until chosen)</span>
        <button class="btn small secondary" :disabled="!undoNext" :title="undoNext ? `Undo: ${undoNext} (Ctrl+Z)` : 'Nothing to undo'" aria-label="Undo" @click="undo">Undo</button>
        <button class="btn small secondary" :disabled="!redoNext" :title="redoNext ? `Redo: ${redoNext} (Ctrl+Shift+Z)` : 'Nothing to redo'" aria-label="Redo" @click="redo">Redo</button>
        <button class="btn small secondary" :disabled="!selectedIds.length" title="Copy the selected blocks and the cables between them (Ctrl+C)" aria-label="Copy blocks" @click="copy">Copy</button>
        <button class="btn small secondary" :disabled="!clipboard.blocks.length" title="Paste here (Ctrl+V)" aria-label="Paste blocks" @click="paste">Paste</button>
        <button class="btn small secondary" :disabled="!selectedIds.length && !getSelectedEdges.length" title="Delete the selected blocks and cables (Delete)" aria-label="Delete selection" @click="deleteSelection">Delete</button>
        <button class="btn small secondary" :disabled="!selectedIds.length" title="Put the selected blocks into a macro" @click="startGroup">Group into macro</button>
        <button class="btn small secondary" :disabled="!selectedInstance" title="Open the selected macro (or double-click it)" aria-label="Open macro" @click="openInstance(selectedInstance)">Open</button>
        <button class="btn small secondary" :disabled="!selectedInstance" title="Replace the selected macro by its blocks" @click="ungroupSelected">Ungroup</button>
        <button class="btn small secondary" @click="fit">Fit</button>
        <span class="lhint muted small">Shift-drag selects an area; Ctrl-click adds a block to the selection.</span>
      </div>
      <div class="llegend small muted">
        Ports:
        <span v-for="t in PORT_TYPES" :key="t" class="ltype"><i :class="`t-${t}`"></i>{{ t }}</span>
      </div>

      <div v-if="grouping" class="lgroup" role="dialog" aria-label="Group into macro">
        <div class="row tight">
          <strong>Group {{ grouping.ids.length }} block{{ grouping.ids.length > 1 ? "s" : "" }} into a macro</strong>
          <input v-model="grouping.name" placeholder="macro name" aria-label="New macro name" style="width: 14rem" />
        </div>
        <p class="note">Choose the ports the macro shows outside. A port a cable from the rest of the graph uses is always shown.</p>
        <div class="lgroupcols">
          <div v-for="side in ['inputs', 'outputs']" :key="side">
            <div class="group-caption">{{ side }}</div>
            <label v-for="p in grouping[side]" :key="`${p.block}.${p.port}`" class="check">
              <input v-model="p.expose" type="checkbox" :disabled="p.crossing" :aria-label="`Expose ${p.block} ${p.port}`" />
              <i class="ldot" :class="`t-${p.type}`"></i>
              {{ p.block }}.{{ p.port }} as
              <input v-model="p.name" :aria-label="`Name of ${p.block} ${p.port}`" style="width: 8rem" />
            </label>
            <div v-if="!grouping[side].length" class="muted small">none</div>
          </div>
        </div>
        <div class="row tight">
          <button class="btn small" :disabled="!!groupProblem" @click="makeGroup">Create macro</button>
          <button class="btn small secondary" @click="grouping = null">Cancel</button>
          <span v-if="groupProblem" class="small" style="color: var(--bad)">{{ groupProblem }}</span>
        </div>
      </div>

      <div v-if="choosing" class="lgroup" role="dialog" aria-label="Choose the target">
        <div class="row tight">
          <strong>{{ typeLabel(choosing.type) }}: choose what it acts on</strong>
          <LogicParam v-for="p in choosing.needs" :key="p.name" :block="choosing.draft" :param="p" :options="optionsOf" @set="choose(p, $event)" />
          <button class="btn small" :disabled="!chosen" @click="addChosen">Add</button>
          <button class="btn small secondary" @click="choosing = null">Cancel</button>
        </div>
      </div>

      <div v-if="confirming" class="lissues lconfirm" role="alertdialog" aria-label="Change a live graph">
        <div><strong>This graph is live.</strong> {{ confirming.text }}</div>
        <div class="row tight">
          <button class="btn small danger" @click="confirmLive(false)">Do it live</button>
          <button class="btn small" @click="confirmLive(true)">Switch to dry run first</button>
          <button class="btn small secondary" @click="confirming = null">Cancel</button>
        </div>
      </div>

      <div v-if="issues.length" class="lissues" role="alert">
        <div class="row tight">
          <strong>Not saved: {{ issues.length }} problem{{ issues.length > 1 ? "s" : "" }}</strong>
          <button class="btn small secondary" @click="dismissIssues">Dismiss</button>
        </div>
        <div v-for="(i, n) in issues" :key="n" class="small" :data-issue-text="i.block || ''">{{ where(i) }}{{ where(i) ? ": " : "" }}{{ i.message }}</div>
      </div>

      <div class="logiceditor">
        <aside class="lpalette" aria-label="Block palette">
          <template v-for="g in palette" :key="g.id">
            <div class="group-caption">{{ g.caption }}</div>
            <button
              v-for="item in g.items"
              :key="item.key"
              class="pitem"
              :class="`k-${g.id}`"
              draggable="true"
              :data-palette="item.key"
              :title="`Drag onto the canvas, or click to add ${item.label}`"
              @dragstart="onPaletteDrag($event, item)"
              @click="addFromPalette(item)"
            >
              {{ item.label }}
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
            :zoom-on-double-click="false"
            :min-zoom="0.2"
            :max-zoom="3"
            class="graph"
            @pane-ready="fit"
            @nodes-change="onNodesChange"
            @edges-change="onEdgesChange"
            @node-drag-stop="onDragStop"
            @node-double-click="openInstance(blockOf($event.node.id))"
            @connect="onConnect"
            @connect-start="onConnectStart"
            @connect-end="drawing = null"
          >
            <template #node-logic="p"><LogicNode :data="p.data" /></template>
            <MiniMap pannable zoomable :width="160" :height="100" />
            <Controls :show-interactive="false" :fit-view-params="FIT" />
          </VueFlow>
        </div>
      </div>

      <div class="ltable">
        <table class="amtable">
          <thead>
            <tr><th>Block</th><th>Type</th><th></th><th>Value</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="b in view.blocks" :key="b.id" :data-block="b.id" :class="{ chosen: selectedIds.includes(b.id), waiting: waiting.has(b.id) }">
              <td>{{ b.id }}</td>
              <td>{{ typeLabel(b.type) }}</td>
              <td class="lparams">
                <LogicParam v-for="p in paramsOf(b)" :key="p.name" :block="b" :param="p" :options="optionsOf" @set="setParam(b, p, $event)" />
                <template v-if="!openMacro">
                  <button v-if="b.type === 'button'" class="btn small" :aria-pressed="pressed(b)" @click="press(b, !pressed(b))">{{ pressed(b) ? "Pressed" : "Press" }}</button>
                  <button v-if="b.type === 'trigger'" class="btn small" :aria-label="`Fire ${b.id}`" @click="fire(b)">Fire</button>
                </template>
                <button v-if="b.type === 'macro'" class="btn small secondary" :aria-label="`Open ${b.id}`" @click="openInstance(b)">Open</button>
              </td>
              <td class="num" :data-value="b.id"><LiveText :block="b" /></td>
              <td><button class="btn small secondary" :aria-label="`Remove ${b.id}`" @click="removeBlock(b.id)">×</button></td>
            </tr>
          </tbody>
        </table>
      </div>

      <div class="row" style="margin-top: 0.4rem">
        <select v-model="addType" aria-label="Block type">
          <option v-for="e in catalog.filter((x) => x.type !== 'macro' && (openMacro || !x.type.startsWith('macro-')))" :key="e.type" :value="e.type">{{ e.label || e.type }}</option>
        </select>
        <select v-model="addSignal" aria-label="Block signal" title="the strip or bus a new block reads or sets">
          <option value="">signal…</option>
          <option v-for="s in signals" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
        <button class="btn small" @click="place(addType)">+ Block</button>
      </div>

      <div class="caption" style="margin-top: 0.8rem">Cables</div>
      <div v-for="w in view.wires" :key="wireKey(w)" class="row" :data-wire="`${w.from_block}-${w.to_block}-${w.to_port}`">
        <span>{{ w.from_block }}.{{ w.from_port }} → {{ w.to_block }}.{{ w.to_port }}</span>
        <button class="btn small secondary" :aria-label="`Remove wire ${w.from_block} to ${w.to_block}`" @click="removeWire(w)">×</button>
      </div>
      <div class="row">
        <select v-model="wire.from" aria-label="Wire from" @change="wire.fromPort = outsOf(wire.from)[0]?.name || ''">
          <option value="">from…</option>
          <option v-for="b in view.blocks.filter((x) => outsOf(x.id).length)" :key="b.id" :value="b.id">{{ b.id }}</option>
        </select>
        <select v-model="wire.fromPort" aria-label="Wire from port">
          <option v-for="p in outsOf(wire.from)" :key="p.name" :value="p.name">{{ p.name }} ({{ p.type }})</option>
        </select>
        <select v-model="wire.to" aria-label="Wire to" @change="wire.port = insOf(wire.to)[0]?.name || ''">
          <option value="">to…</option>
          <option v-for="b in view.blocks.filter((x) => insOf(x.id).length)" :key="b.id" :value="b.id">{{ b.id }}</option>
        </select>
        <select v-model="wire.port" aria-label="Wire port">
          <option v-for="p in insOf(wire.to)" :key="p.name" :value="p.name">{{ p.name }} ({{ p.type }})</option>
        </select>
        <button class="btn small" @click="addWire">+ Wire</button>
      </div>

      <template v-if="!openMacro">
        <div class="caption" style="margin-top: 0.8rem">Actions{{ graph.dry_run ? " (dry run: not done)" : "" }}</div>
        <div v-if="!detail.actions.length" class="muted small">None yet.</div>
        <div v-for="(a, i) in [...detail.actions].reverse()" :key="i" class="small lact" :data-action="a.block">
          <span class="muted num">{{ when(a.t_ms) }}</span> {{ a.block }}: {{ a.action }}<span v-if="a.dry_run" class="pill warn">dry run</span>
        </div>
      </template>
    </template>
  </div>
</template>
