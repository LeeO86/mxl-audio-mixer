// The processing clipboard (B1): units copied from a chain, kept in this browser (all its tabs), pasted
// into another chain as a whole ("paste all"), one by one, or onto a unit of the same type (its
// parameters, keeping its id and place).
import { reactive } from "vue";
import { store, stored } from "./store.js";

const KEY = "chain.clipboard";
const load = () => {
  try {
    const v = JSON.parse(stored(KEY, "[]"));
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
};
export const clipboard = reactive({ units: load() });
const save = () => store(KEY, JSON.stringify(clipboard.units));
// Another tab copied: follow it.
if (typeof window !== "undefined")
  window.addEventListener("storage", (ev) => {
    if (ev.key?.endsWith(KEY)) clipboard.units = load();
  });

/** A unit as the clipboard keeps it: everything but its id, with where it came from. */
export function unitOf(node, from) {
  const { id, ...unit } = node;
  return { unit: JSON.parse(JSON.stringify(unit)), from, label: id };
}

export function copy(node, from) {
  clipboard.units.push(unitOf(node, from));
  save();
}
export function forget(i) {
  clipboard.units.splice(i, 1);
  save();
}
export function clear() {
  clipboard.units = [];
  save();
}

/** The last copied unit of `type`, or null. */
export function latestOf(type) {
  for (let i = clipboard.units.length - 1; i >= 0; i--) if (clipboard.units[i].unit.type === type) return clipboard.units[i];
  return null;
}

/** The merge patch that turns `node` into `unit` (same type): its fields, and null for the ones it lacks. */
export function replacePatch(node, unit) {
  const patch = { ...unit };
  for (const k of Object.keys(node)) if (k !== "id" && !(k in unit)) patch[k] = null;
  return patch;
}
