// Logic editor helpers (§11.3): the undo history, the clipboard and the id and wire bookkeeping for
// copy and paste. Pure functions except the two module-level stores, which outlive the Logic tab so
// that undo and the clipboard survive switching tabs and graphs.
import { reactive } from "vue";

/** What an edit changes: a graph's blocks and wires (name, on and dry run are settings, not edits). */
export const edited = (g) => ({ blocks: g?.blocks || [], wires: g?.wires || [] });

// JSON with sorted keys, so two graphs compare equal whatever order the server writes fields in.
function canon(v) {
  if (Array.isArray(v)) return `[${v.map(canon).join(",")}]`;
  if (v && typeof v === "object") return `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${canon(v[k])}`).join(",")}}`;
  return JSON.stringify(v);
}
export const sameGraph = (a, b) => canon(edited(a)) === canon(edited(b));
// A plain copy (structuredClone refuses Vue's reactive proxies).
const clone = (v) => JSON.parse(JSON.stringify(v));

// ---- undo / redo ------------------------------------------------------------------------------
// One history per graph: each entry is the graph's blocks and wires before and after one edit, as the
// server answered them. Undo writes `before` back only while the server still has `after`; if anyone
// else changed the graph meanwhile, the history of that graph is dropped instead of overwriting them.

const LIMIT = 100;
export const history = reactive({}); // graph id -> { undo: [entry], redo: [entry] }

export function historyOf(id) {
  return (history[id] ||= { undo: [], redo: [] });
}

export function record(id, label, before, after) {
  const h = historyOf(id);
  h.undo.push({ label, before: clone(edited(before)), after: clone(edited(after)) });
  if (h.undo.length > LIMIT) h.undo.shift();
  h.redo = [];
}

export function forget(id) {
  delete history[id];
}

// ---- copy / paste -----------------------------------------------------------------------------

export const clipboard = reactive({ blocks: [], wires: [], pastes: 0 });

/** The blocks with these ids and the wires between them. */
export function copyOf(g, ids) {
  const keep = new Set(ids);
  return {
    blocks: clone(g.blocks.filter((b) => keep.has(b.id))),
    wires: clone(g.wires.filter((w) => keep.has(w.from_block) && keep.has(w.to_block))),
  };
}

/** `type-n` with the lowest n not in `taken`. */
export function freshId(type, taken) {
  for (let n = 1; ; n++) if (!taken.has(`${type}-${n}`)) return `${type}-${n}`;
}

/** The clip with new ids that do not clash with `g`, moved by `offset`; its wires follow the new ids. */
export function pasted(g, clip, offset) {
  const taken = new Set(g.blocks.map((b) => b.id));
  const rename = {};
  const blocks = clip.blocks.map((b) => {
    const id = freshId(b.type, taken);
    taken.add(id);
    rename[b.id] = id;
    return { ...clone(b), id, x: Math.round((b.x || 0) + offset), y: Math.round((b.y || 0) + offset) };
  });
  const wires = clip.wires.map((w) => ({ ...w, from_block: rename[w.from_block], to_block: rename[w.to_block] }));
  return { blocks, wires, ids: blocks.map((b) => b.id) };
}
