// Virtual outputs (C7): fixed MXL flows and NMOS senders the designer wires (MIXER_OUTPUT_FLOWS); the
// operator picks what feeds each. A source is { signal, tap? } (a strip's direct out or a bus),
// { monitor } (a control-room monitor) or { input, format? } (an input straight through).
import { computed } from "vue";
import { api } from "./api.js";
import { act, buses, inputName, isBus, live, room, signalOf, strips } from "./store.js";

export const vouts = computed(() => live.status?.virtual_outputs || []);

/** What a source is, in words. */
export function sourceLabel(src) {
  if (!src) return "nothing (silence)";
  if ("signal" in src) {
    const s = signalOf(src.signal);
    const what = isBus(src.signal) ? "bus" : "direct out";
    return `${s?.name || src.signal} (${what}${src.tap && src.tap !== "post-fader" ? `, ${typeof src.tap === "object" ? `after ${src.tap.after}` : src.tap}` : ""})`;
  }
  if ("monitor" in src) return `${room.value.monitors.find((m) => m.id === src.monitor)?.name || src.monitor} (monitor)`;
  const i = (live.status?.inputs || []).find((x) => x.input === src.input);
  return `${i ? inputName(i) : `Input ${src.input}`} (input ${src.input}${src.format ? `, as ${src.format}` : ""})`;
}

/** Every source a virtual output can take, as [key, source, label]; the key is the source as JSON. */
export const sourceChoices = computed(() => {
  const out = [];
  const add = (src) => out.push([JSON.stringify(src), src, sourceLabel(src)]);
  for (const b of buses.value) add({ signal: b.id });
  for (const s of strips.value) add({ signal: s.id });
  for (const m of room.value.monitors || []) add({ monitor: m.id });
  for (const i of live.status?.inputs || []) add({ input: i.input });
  return out;
});

/** Feed virtual output `slot` from `source` (null: silence). */
export const setVoutSource = (slot, source) => act(() => api.put(`/api/v1/virtual-outputs/${slot}`, { source }));
