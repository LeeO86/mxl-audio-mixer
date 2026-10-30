// Logic editor helpers (§11.3): typed ports and the wiring rule, the undo history, the clipboard, and
// grouping blocks into a macro and back. Pure functions except the two module-level stores, which
// outlive the Logic tab so that undo and the clipboard survive switching tabs and graphs.
import { markRaw, reactive } from "vue";

// A plain copy (structuredClone refuses Vue's reactive proxies).
export const clone = (v) => JSON.parse(JSON.stringify(v));

// ---- ports ------------------------------------------------------------------------------------

export const PORT_TYPES = ["boolean", "number", "level", "event", "string"];

/** D158: an input converts what it gets, except a string into anything else and an event into a string. */
export const canWire = (from, to) => (from === "string" ? to === "string" : !(from === "event" && to === "string"));

/**
 * A block's ports, `{inputs: [{name, type}], outputs: [...]}`: from the catalog (`GET
 * /api/v1/logic-blocks`), except the blocks whose ports follow their parameters: `select`, `macro-in`,
 * `macro-out` and a macro instance (its macro's `macro-in` and `macro-out` blocks).
 */
export function portsOf(b, catalog, macros) {
  const t = b.value_type || "boolean";
  if (b.type === "macro-in") return { inputs: [], outputs: [{ name: "out", type: t }] };
  if (b.type === "macro-out") return { inputs: [{ name: "in", type: t }], outputs: [] };
  if (b.type === "select") {
    const n = Number(b.inputs) || 2;
    return { inputs: [{ name: "sel", type: "number" }, ...Array.from({ length: n }, (_, i) => ({ name: `in${i}`, type: t }))], outputs: [{ name: "out", type: t }] };
  }
  if (b.type === "macro") {
    const m = macros.find((x) => x.id === b.macro);
    const side = (type, port) => (m?.blocks || []).filter((x) => x.type === type).map((x) => ({ name: x.name, type: x.value_type || "boolean", port }));
    return { inputs: side("macro-in"), outputs: side("macro-out") };
  }
  const e = catalog.find((x) => x.type === b.type);
  return { inputs: e?.inputs || [], outputs: e?.outputs || [] };
}

// ---- resources, revisions and the undo history ------------------------------------------------

// What GET adds to a graph or macro; a write takes the rest.
const ADDED = ["values", "ports", "buttons", "actions", "revision", "warnings", "exposed"];
export function resource(r) {
  if (!r) return null;
  const out = clone(r);
  for (const k of ADDED) delete out[k];
  return out;
}

// JSON with sorted keys: two copies of a resource compare equal whatever order the server writes.
function canon(v) {
  if (Array.isArray(v)) return `[${v.map(canon).join(",")}]`;
  if (v && typeof v === "object") return `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${canon(v[k])}`).join(",")}}`;
  return JSON.stringify(v);
}
export const same = (a, b) => canon(resource(a)) === canon(resource(b));
// The same edit: equal but for "on" and "dry run", which are the operator's and never undone (D175).
const edit = (r) => {
  const o = resource(r);
  if (o) delete o.enabled, delete o.dry_run;
  return o;
};
export const sameEdit = (a, b) => canon(edit(a)) === canon(edit(b));

// One history per document ("graph:<id>" or "macro:<id>"). An entry is one edit: for each graph or
// macro it touched, the resource before and after (null: it did not exist). Undo reads each one again
// and goes ahead only while it is still what this client left (D175).
const LIMIT = 100;
export const history = reactive({}); // doc key -> { undo: [entry], redo: [entry] }

export function historyOf(key) {
  return (history[key] ||= { undo: [], redo: [] });
}

export function record(key, entry) {
  const h = historyOf(key);
  h.undo.push(markRaw(entry));
  if (h.undo.length > LIMIT) h.undo.shift();
  h.redo = [];
}

export function forget(key) {
  delete history[key];
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

/** The clip moved by `offset`; a block whose id `g` has already gets a new one (D245: the others keep
 * theirs, so `POST …/trigger/<block>` still finds a copied trigger), and its wires follow. */
export function pasted(g, clip, offset) {
  const taken = new Set(g.blocks.map((b) => b.id));
  const rename = {};
  const blocks = clip.blocks.map((b) => {
    const id = taken.has(b.id) ? freshId(b.type, taken) : b.id;
    taken.add(id);
    rename[b.id] = id;
    return { ...clone(b), id, x: Math.round((b.x || 0) + offset), y: Math.round((b.y || 0) + offset) };
  });
  const wires = clip.wires.map((w) => ({ ...w, from_block: rename[w.from_block], to_block: rename[w.to_block] }));
  return { blocks, wires, ids: blocks.map((b) => b.id) };
}

// ---- macros (D163) ----------------------------------------------------------------------------

/** A port name the server takes in an id (`in-<name>`): a–z, 0–9 and dashes, at most 28 characters. */
export const portName = (s) =>
  String(s)
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .slice(0, 28)
    .replace(/^-+|-+$/g, "") || "port";

/**
 * The ports a group of blocks can expose: every input of the group not fed from inside it and every
 * output. `crossing` marks the ones a cable from or to the rest of the graph uses: those must be exposed.
 */
export function boundary(g, ids, ports) {
  const inside = new Set(ids);
  const inputs = [];
  const outputs = [];
  for (const b of g.blocks.filter((x) => inside.has(x.id))) {
    const p = ports(b);
    for (const i of p.inputs) {
      const feed = g.wires.find((w) => w.to_block === b.id && w.to_port === i.name);
      if (feed && inside.has(feed.from_block)) continue;
      inputs.push({ block: b.id, port: i.name, type: i.type, crossing: !!feed });
    }
    for (const o of p.outputs) {
      const crossing = g.wires.some((w) => w.from_block === b.id && w.from_port === o.name && !inside.has(w.to_block));
      outputs.push({ block: b.id, port: o.name, type: o.type, crossing });
    }
  }
  // Names: the port's own when it is unique, else block and port.
  const named = (list) => {
    const count = {};
    for (const p of list) count[p.port] = (count[p.port] || 0) + 1;
    return list.map((p) => ({ ...p, name: portName(count[p.port] > 1 ? `${p.block}-${p.port}` : p.port), expose: p.crossing }));
  };
  return { inputs: named(inputs), outputs: named(outputs) };
}

/**
 * Groups `ids` of graph (or macro) `g` into a new macro `macroId` named `name`, exposing the chosen
 * ports (`exposed`: boundary() entries with `expose` and `name`). Returns the macro and the graph's new
 * blocks and wires, where an instance `instanceId` replaces the group and takes over its cables.
 */
export function group(g, ids, exposed, { macroId, name, instanceId }) {
  const inside = new Set(ids);
  const members = clone(g.blocks.filter((b) => inside.has(b.id)));
  const ins = exposed.inputs.filter((p) => p.expose);
  const outs = exposed.outputs.filter((p) => p.expose);
  const minX = Math.min(...members.map((b) => b.x || 0));
  const minY = Math.min(...members.map((b) => b.y || 0));
  const maxX = Math.max(...members.map((b) => b.x || 0));
  const inner = members.map((b) => ({ ...b, x: Math.round((b.x || 0) - minX + 260), y: Math.round((b.y || 0) - minY + 40) }));
  const width = maxX - minX + 260;
  const macroIn = ins.map((p, i) => ({ id: `in-${p.name}`, type: "macro-in", name: p.name, value_type: p.type, x: 0, y: 40 + i * 90 }));
  const macroOut = outs.map((p, i) => ({ id: `out-${p.name}`, type: "macro-out", name: p.name, value_type: p.type, x: width + 260, y: 40 + i * 90 }));
  const wires = [
    ...clone(g.wires.filter((w) => inside.has(w.from_block) && inside.has(w.to_block))),
    ...ins.map((p) => ({ from_block: `in-${p.name}`, from_port: "out", to_block: p.block, to_port: p.port })),
    ...outs.map((p) => ({ from_block: p.block, from_port: p.port, to_block: `out-${p.name}`, to_port: "in" })),
  ];
  const macro = { id: macroId, name, description: "", blocks: [...macroIn, ...inner, ...macroOut], wires };

  const inName = (b, port) => ins.find((p) => p.block === b && p.port === port)?.name;
  const outName = (b, port) => outs.find((p) => p.block === b && p.port === port)?.name;
  const outer = [];
  for (const w of g.wires) {
    const a = inside.has(w.from_block);
    const z = inside.has(w.to_block);
    if (!a && !z) outer.push(w);
    else if (!a && z && inName(w.to_block, w.to_port)) outer.push({ ...w, to_block: instanceId, to_port: inName(w.to_block, w.to_port) });
    else if (a && !z && outName(w.from_block, w.from_port)) outer.push({ ...w, from_block: instanceId, from_port: outName(w.from_block, w.from_port) });
  }
  const instance = { id: instanceId, type: "macro", macro: macroId, x: Math.round(minX), y: Math.round(minY) };
  return { macro, blocks: [...g.blocks.filter((b) => !inside.has(b.id)), instance], wires: outer };
}

/**
 * Replaces instance `instanceId` of `g` with its macro's blocks (ids `<instance>-<block>` when free and
 * at most 32 characters, else `<type>-<n>`), wired as the macro wires them, and its cables connected
 * through to them.
 */
export function ungroup(g, instanceId, macro) {
  const inst = g.blocks.find((b) => b.id === instanceId);
  const real = macro.blocks.filter((b) => b.type !== "macro-in" && b.type !== "macro-out");
  const taken = new Set(g.blocks.map((b) => b.id).filter((id) => id !== instanceId));
  const rename = {};
  for (const b of real) {
    let id = `${instanceId}-${b.id}`;
    if (taken.has(id) || id.length > 32) id = freshId(b.type, taken);
    taken.add(id);
    rename[b.id] = id;
  }
  const minX = Math.min(...real.map((b) => b.x || 0));
  const minY = Math.min(...real.map((b) => b.y || 0));
  const blocks = real.map((b) => ({ ...clone(b), id: rename[b.id], x: Math.round((b.x || 0) - minX + (inst.x || 0)), y: Math.round((b.y || 0) - minY + (inst.y || 0)) }));
  const kind = Object.fromEntries(macro.blocks.map((b) => [b.id, b]));
  const wires = [];
  for (const w of g.wires) if (w.from_block !== instanceId && w.to_block !== instanceId) wires.push(w);
  for (const w of macro.wires) {
    const from = kind[w.from_block];
    const to = kind[w.to_block];
    if (from?.type === "macro-in") {
      // The cable into the instance's input, if any, now feeds this inner input.
      const feed = g.wires.find((x) => x.to_block === instanceId && x.to_port === from.name);
      if (feed && to?.type !== "macro-out") wires.push({ ...feed, to_block: rename[w.to_block], to_port: w.to_port });
      if (feed && to?.type === "macro-out") for (const x of g.wires.filter((y) => y.from_block === instanceId && y.from_port === to.name)) wires.push({ ...x, from_block: feed.from_block, from_port: feed.from_port });
    } else if (to?.type === "macro-out") {
      for (const x of g.wires.filter((y) => y.from_block === instanceId && y.from_port === to.name)) wires.push({ ...x, from_block: rename[w.from_block], from_port: w.from_port });
    } else {
      wires.push({ ...w, from_block: rename[w.from_block], to_block: rename[w.to_block] });
    }
  }
  return { blocks: [...g.blocks.filter((b) => b.id !== instanceId), ...blocks], wires, ids: blocks.map((b) => b.id) };
}
