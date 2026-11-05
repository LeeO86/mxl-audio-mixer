// Strip inputs besides the MXL inputs and talkback (D1, 1.6.0): every bus (its bus out), every strip's
// direct out (post-fader, the published tap, and each direct-out block of its chain) and every virtual
// output, by their stable ids, channel by channel. They are read in the same block, without extra delay.
// The server refuses a choice that would close a feedback loop and says which loop; the list marks those
// it can see coming. A patch point here is { signal, tap?, channel } or { output, channel }.
import { channelLabels } from "./console.js";
import { buses, live, reaches, strips } from "./store.js";

const tapText = (tap) => (!tap ? "" : typeof tap === "object" ? `after=${tap.after}` : tap);

/** The `<option>` value of a signal or virtual-output patch point ("" for others). */
export function signalPatchKey(p) {
  if (!p) return "";
  if ("signal" in p) return `s:${p.signal}|${tapText(p.tap)}|${p.channel}`;
  if ("output" in p) return `o:${p.output}|${p.channel}`;
  return "";
}

/** The patch point of a value from {@link signalPatchKey}, or null. */
export function signalPatchOf(value) {
  if (value?.startsWith("s:")) {
    const [signal, tap, channel] = value.slice(2).split("|");
    const point = { signal, channel: Number(channel) };
    if (tap.startsWith("after=")) point.tap = { after: tap.slice(6) };
    else if (tap) point.tap = tap;
    return point;
  }
  if (value?.startsWith("o:")) {
    const [output, channel] = value.slice(2).split("|");
    return { output, channel: Number(channel) };
  }
  return null;
}

/** Short text of a signal or virtual-output patch point. */
export function signalPatchText(p) {
  const name = (id) => [...strips.value, ...buses.value].find((s) => s.id === id)?.name || id;
  if ("signal" in p) return `${name(p.signal)}${p.tap ? ` ${typeof p.tap === "object" ? p.tap.after : p.tap}` : ""}·${p.channel}`;
  return `out ${p.output}·${p.channel}`;
}

/** The signal a point reads from the mix (a virtual output: what feeds it), or null. */
function readSignal(p) {
  if ("signal" in p) return p.signal;
  const v = (live.status?.virtual_outputs || []).find((o) => (o.key || String(o.slot)) === p.output);
  return v?.source && "signal" in v.source ? v.source.signal : null;
}

/**
 * The sources strip `strip` can read besides inputs and talkback: [{ label, points: [{ value, text,
 * point, disabled, title }] }], one group per source with an entry per channel. A source the strip feeds
 * (or the strip itself) is disabled: it would close a loop.
 */
export function signalSources(strip) {
  const groups = [];
  const add = (label, channels, point, loops) => {
    const title = loops ? "would close a feedback loop (refused)" : "";
    groups.push({
      label,
      points: Array.from({ length: channels }, (_, c) => {
        const p = point(c + 1);
        return { value: signalPatchKey(p), text: `${label}·${c + 1}`, point: p, disabled: loops, title };
      }),
    });
  };
  const loops = (signal) => !!signal && (signal === strip || reaches(strip, signal));
  for (const b of buses.value) add(`${b.name} (bus)`, channelLabels(b.format).length, (channel) => ({ signal: b.id, channel }), loops(b.id));
  for (const s of strips.value) {
    const n = channelLabels(s.format).length;
    add(`${s.name} (direct out)`, n, (channel) => ({ signal: s.id, channel }), loops(s.id));
    // The published output's own tap, and the direct-out blocks of its chain (C5), by their ids.
    const published = (live.mix?.outputs || []).find((o) => o.signal === s.id && o.tap && o.tap !== "post-fader");
    const taps = [...(published ? [published.tap] : []), ...(s.chain || []).filter((p) => p.type === "direct-out").map((p) => ({ after: p.id }))];
    for (const tap of taps) add(`${s.name} (${tapText(tap).replace("after=", "after ")})`, n, (channel) => ({ signal: s.id, tap, channel }), loops(s.id));
  }
  for (const v of live.status?.virtual_outputs || []) {
    const output = v.key || String(v.slot);
    add(`${v.label || output} (virtual output)`, channelLabels(v.format).length, (channel) => ({ output, channel }), loops(readSignal({ output })));
  }
  return groups;
}
