// The taps a strip or bus offers, from the server (GET /api/v1/taps, the same check as its validation),
// so a tap list never offers one the server refuses (a bus has no input or post-pan). Fetched again
// after every mix change while a view asks for them; a failed fetch empties the list and is retried
// after 2 s; a reconnect (perhaps a restarted mixer, whose revisions start again) drops the list.
import { reactive, watch } from "vue";
import { api } from "./api.js";
import { live } from "./store.js";
import { TAPS } from "./nodes.js";

const state = reactive({ revision: null, list: {} });
let loading = null;
let retry = 0;
let wanted = false;

function drop() {
  Object.assign(state, { revision: null, list: {} });
}

watch(
  () => live.connected,
  () => {
    drop();
    if (wanted) loadTaps();
  },
);

/** Fetch the taps unless they are those of the current mix revision. */
export async function loadTaps() {
  wanted = true;
  if (state.revision === live.revision || loading || retry) return;
  const revision = live.revision;
  let ok = false;
  loading = api
    .get("/api/v1/taps")
    .then((list) => {
      Object.assign(state, { revision, list });
      ok = true;
    })
    .catch(() => {
      drop();
      retry = setTimeout(() => {
        retry = 0;
        loadTaps();
      }, 2000);
    })
    .finally(() => (loading = null));
  await loading;
  // The mix changed while the answer was on its way.
  if (ok && state.revision !== live.revision) loadTaps();
}

/** Select options [value, label] of `id`'s taps ("after:<node>" for a node); `current` stays listed. */
export function tapOptions(id, current) {
  const list = state.list[id] || [];
  const opts = list.map((t) => (typeof t === "object" ? [`after:${t.after}`, `after ${t.after}`] : [t, TAPS.find((x) => x[0] === t)?.[1] || t]));
  if (current && !opts.some(([v]) => v === current)) opts.push([current, current.startsWith("after:") ? `after ${current.slice(6)}` : TAPS.find((x) => x[0] === current)?.[1] || current]);
  return opts;
}
