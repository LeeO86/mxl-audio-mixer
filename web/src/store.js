// Shared UI state: the status, the mix and the meters from the WebSocket /api/v1/events (README "Live
// updates"), the configuration, and the actions every view uses. Every client sees every change: the
// mix comes back on the WebSocket after a change from any client.
import { computed, reactive } from "vue";
import { api, exchange } from "./api.js";

const PREFIX = "mxl-audio-mixer.";
export function stored(key, fallback) {
  try {
    return localStorage.getItem(PREFIX + key) ?? fallback;
  } catch {
    return fallback;
  }
}
export function store(key, value) {
  try {
    localStorage.setItem(PREFIX + key, String(value));
  } catch {
    /* kept for this tab only */
  }
}

export const live = reactive({
  status: null, // GET /api/v1/status
  mix: null, // GET /api/v1/mix
  revision: -1,
  latency: {}, // signal -> post-fader processing latency in ms
  meters: {}, // GET /api/v1/meters
  loudness: {}, // GET /api/v1/loudness
  vad: {}, // analysed signal -> { active, probability } (§8.1)
  autoDb: {}, // signal -> its auto node's gain in dB (its deepest channel), null for −∞ (§8)
  autolevels: {}, // signal -> { gain_db, level_lufs, target_lufs, frozen, speech } (§8.2)
  duckControl: {}, // ratio duck id -> { state, reduction_db, ratio_lu, target_lu, … } (§8.4)
  analysis: null, // the analyser's values while a view wants them
  config: null, // GET /api/v1/config
  connected: false,
  everConnected: false,
  error: "", // API unreachable
  actionError: "", // the last failed action (banner)
  notice: "", // the last action's result worth showing (banner)
  selected: stored("selected", ""), // the signal the processing view edits
});

export const strips = computed(() => live.mix?.strips || []);
export const buses = computed(() => live.mix?.buses || []);
export const sends = computed(() => live.mix?.sends || []);
export const signals = computed(() => [...strips.value, ...buses.value]);
export const signalOf = (id) => strips.value.find((s) => s.id === id) || buses.value.find((b) => b.id === id) || null;
export const isBus = (id) => buses.value.some((b) => b.id === id);
export const sendOf = (from, to) => sends.value.find((s) => s.from === from && s.to === to) || null;
export const settingValue = (key) => live.config?.settings?.find((s) => s.key === key)?.value ?? "";

export function select(id) {
  live.selected = id;
  store("selected", id);
}

/** Runs an action; a failure goes to the error banner. Returns the result (true for an empty answer), or undefined. */
export async function act(fn, notice = "") {
  try {
    const result = await fn();
    live.actionError = "";
    if (notice) live.notice = notice;
    return result ?? true;
  } catch (e) {
    live.actionError = e.message;
    return undefined;
  }
}

// ---- parameter changes -------------------------------------------------------------
// A fader or pot being moved sends at most 30 PATCHes a second per path; changes to one path merge, so
// a fast second control does not drop the first. One request per path at a time: two in flight could
// reach the server in the wrong order and leave the older value.

const pending = new Map(); // "METHOD path" -> body
const timers = new Map(); // key -> timeout
const lastSend = new Map(); // key -> time
const inFlight = new Set(); // keys
// Local changes not yet in a pushed mix: key -> { target: () => object, body, until: revision }. A mix
// pushed before the server applied them (still on its way) would otherwise undo them for a moment, and
// a toggle pressed then would toggle from the old value.
const unacked = new Map();

function flushKey(key) {
  clearTimeout(timers.get(key));
  timers.delete(key);
  if (inFlight.has(key)) return;
  const body = pending.get(key);
  pending.delete(key);
  if (!body) return;
  lastSend.set(key, performance.now());
  inFlight.add(key);
  const [method, path] = key.split(" ");
  act(() => exchange(path, { method, body })).then((ok) => {
    inFlight.delete(key);
    if (ok === undefined) {
      // Refused: the server's mix did not change, so take it back over the local change.
      unacked.delete(key);
      refreshMix();
    } else if (!pending.has(key) && unacked.has(key)) {
      if (ok.revision === null || ok.revision <= live.revision) unacked.delete(key);
      else unacked.get(key).until = ok.revision;
    }
    if (pending.has(key)) schedule(key);
  });
}

function schedule(key) {
  const wait = 33 - (performance.now() - (lastSend.get(key) || 0));
  if (wait <= 0) flushKey(key);
  else if (!timers.has(key)) timers.set(key, setTimeout(() => flushKey(key), wait));
}

async function refreshMix() {
  try {
    live.mix = await api.get("/api/v1/mix");
  } catch {
    /* the banner tells */
  }
}

/** PATCH (or PUT) `path` with `body`, rate-limited and merged per path. */
export function patch(path, body, method = "PATCH") {
  const key = `${method} ${path}`;
  pending.set(key, { ...(pending.get(key) || {}), ...body });
  schedule(key);
}

// Changes show at once in this client and stay over pushed mixes until one has them, so a toggle
// pressed twice quickly toggles twice.
function assign(target, body) {
  if (!target) return;
  for (const [k, v] of Object.entries(body)) if (k !== "ramp_ms") target[k] = v;
}
function local(key, target, body) {
  assign(target(), body);
  unacked.set(key, { target, body: { ...(unacked.get(key)?.body || {}), ...body }, until: Infinity });
}
function changeLocally(target, path, body, method = "PATCH") {
  local(`${method} ${path}`, target, body);
  patch(path, body, method);
}
export const signalPath = (id) => `/api/v1/${isBus(id) ? "buses" : "strips"}/${encodeURIComponent(id)}`;
export function patchSignal(id, body) {
  changeLocally(() => signalOf(id), signalPath(id), body);
}
export function patchSend(from, to, body) {
  changeLocally(() => sendOf(from, to), `/api/v1/sends/${encodeURIComponent(from)}/${encodeURIComponent(to)}`, body, "PUT");
}
export function patchNode(id, node, body) {
  changeLocally(() => signalOf(id)?.chain?.find((n) => n.id === node), `${signalPath(id)}/chain/${encodeURIComponent(node)}`, body);
}

/** Send everything pending now (before an action that depends on it). */
export function flush() {
  for (const key of [...pending.keys()]) flushKey(key);
}

// ---- live connection --------------------------------------------------------------

const subscription = { meters: null, meter_hz: 25, analysis: false };
let socket = null;
let retry = 0;
let timer = 0;

function subscribe() {
  if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "subscribe", ...subscription }));
}

/** Only these signals' meters (null: all), e.g. for a widget. */
export function wantMeters(ids) {
  subscription.meters = ids;
  subscribe();
}

/** Who is soloed: { signal: "pfl" | "afl" }. */
export const soloed = computed(() => live.status?.solo || {});
/** Solo `id` (kind "pfl" or "afl"), or end its solo when it already is soloed so. */
export function toggleSolo(id, kind) {
  const on = soloed.value[id] === kind;
  if (live.status) live.status.solo = { ...soloed.value, [id]: on ? undefined : kind };
  return act(() => (on ? api.del(`/api/v1/solo/${encodeURIComponent(id)}`) : api.put(`/api/v1/solo/${encodeURIComponent(id)}`, { kind })));
}
export const clearSolo = () => act(() => api.del("/api/v1/solo"));
export const room = computed(() => live.mix?.control_room || { monitors: [], sources: [], solo: {}, reference_spl: [] });
export const patchMonitor = (id, body) => {
  changeLocally(() => room.value.monitors.find((m) => m.id === id), `/api/v1/monitors/${encodeURIComponent(id)}`, body);
};

// ---- VCAs and mute groups (§9) ----------------------------------------------------------
export const vcas = computed(() => live.mix?.vcas || []);
export const muteGroups = computed(() => live.mix?.mute_groups || []);
export const vcaOf = (id) => vcas.value.find((v) => v.id === id) || null;
export const patchVca = (id, body) => changeLocally(() => vcaOf(id), `/api/v1/vcas/${encodeURIComponent(id)}`, body);
export const patchMuteGroup = (id, body) =>
  changeLocally(() => muteGroups.value.find((g) => g.id === id), `/api/v1/mute-groups/${encodeURIComponent(id)}`, body);
/** Every VCA above `id` (through nested VCAs). */
export function vcasAbove(id) {
  const found = new Set();
  const todo = [id];
  while (todo.length) {
    const m = todo.pop();
    for (const v of vcas.value) if (v.members.includes(m) && !found.has(v.id)) found.add(v.id), todo.push(v.id);
  }
  return [...found].map(vcaOf).filter(Boolean);
}
/** The strips and buses under VCA `id` (through nested VCAs). */
export function vcaSignals(id) {
  const out = new Set();
  const seen = new Set();
  const todo = [id];
  while (todo.length) {
    const v = vcaOf(todo.pop());
    if (!v || seen.has(v.id)) continue;
    seen.add(v.id);
    for (const m of v.members) (vcaOf(m) ? todo.push(m) : out.add(m));
  }
  return [...out];
}
/** Muted by a VCA or an active mute group (its own mute untouched). */
export const vcaMuted = (id) => vcasAbove(id).some((v) => v.mute);
export const groupMuted = (s) => !s.mute_safe && muteGroups.value.some((g) => g.active && g.members.includes(s.id));
/** The console shows only these signals (a VCA's spill), or everything with null. */
export const spill = reactive({ vca: null });

// ---- VAD, auto-mixes and ducks (§8) ------------------------------------------------------
export const automixes = computed(() => live.mix?.automixes || []);
export const ducks = computed(() => live.mix?.ducks || []);
export const automixOf = (id) => automixes.value.find((a) => a.members.some((m) => m.signal === id)) || null;
export const patchAutomix = (id, body) =>
  changeLocally(() => automixes.value.find((a) => a.id === id), `/api/v1/automixes/${encodeURIComponent(id)}`, body);
// A duck's `ratio` and `channel_depth` are objects: the local change merges into them, as the API does.
export const patchDuck = (id, body) => {
  const d = ducks.value.find((x) => x.id === id);
  const merged = { ...body };
  for (const k of ["ratio", "channel_depth"]) if (body[k] && d) merged[k] = { ...d[k], ...body[k] };
  changeLocally(() => ducks.value.find((x) => x.id === id), `/api/v1/ducks/${encodeURIComponent(id)}`, merged);
};
export const autolevels = computed(() => live.mix?.autolevels || []);
export const putAutolevel = (signal, body) =>
  changeLocally(() => autolevels.value.find((l) => l.signal === signal), `/api/v1/autolevels/${encodeURIComponent(signal)}`, body, "PUT");
/** Every analysed signal: its own VAD settings, auto-mix members, duck keys, the targets of ratio
 * ducks that hear them, auto-levelled signals. */
export const vadSignals = computed(() => {
  const ids = new Set((live.mix?.vad || []).map((v) => v.signal));
  for (const a of automixes.value) for (const m of a.members) ids.add(m.signal);
  for (const d of ducks.value) for (const k of d.keys) ids.add(k);
  for (const d of ducks.value) if (d.mode === "ratio" && d.ratio?.bed_voice) for (const t of d.targets) ids.add(t);
  for (const l of autolevels.value) ids.add(l.signal);
  return [...ids];
});
/** A signal's auto node gain in dB (−Infinity for −∞), or undefined without one. */
export function autoGain(id) {
  if (!(id in live.autoDb)) return undefined;
  const v = live.autoDb[id];
  return v === null ? -Infinity : v;
}

/** The tally of a strip: { index, on_air, preview, afv, afv_open }, or null. */
export const tallyOf = (id) => live.status?.tally?.state?.strips?.[id] || null;

/** Ask for the analyser's values (spectrum, goniometer) or stop them. */
export function wantAnalysis(on) {
  subscription.analysis = on;
  if (!on) live.analysis = null;
  subscribe();
}

export async function refreshStatus() {
  try {
    live.status = await api.get("/api/v1/status");
    if (live.revision < 0) live.mix = await api.get("/api/v1/mix");
    live.error = "";
  } catch (e) {
    live.error = `API unreachable: ${e.message}`;
  }
}

export async function refreshConfig() {
  try {
    live.config = await api.get("/api/v1/config");
  } catch {
    /* keep the last answer; the connection banner tells */
  }
}

function onMessage(m) {
  if (m.type === "status") live.status = m.status;
  else if (m.type === "mix") {
    // A mix older than the one shown (a push that was overtaken, or this client's own change already
    // taken from its answer) is dropped.
    if (m.revision < live.revision) return;
    live.mix = m.mix;
    live.revision = m.revision;
    for (const [key, u] of unacked) {
      if (m.revision >= u.until) unacked.delete(key);
      else assign(u.target(), u.body);
    }
    live.latency = m.latency_ms || {};
  } else if (m.type === "meters") {
    live.meters = m.meters;
    live.loudness = m.loudness;
    live.vad = m.vad || {};
    live.autoDb = m.auto_db || {};
    live.autolevels = m.autolevels || {};
    live.duckControl = m.ducks || {};
  } else if (m.type === "analysis") live.analysis = m;
}

function connect() {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  socket = new WebSocket(`${proto}//${location.host}/api/v1/events`);
  socket.onopen = () => {
    // A restarted mixer counts revisions from the start again: take the first push as it comes.
    live.revision = -1;
    live.connected = true;
    live.everConnected = true;
    live.error = "";
    subscribe();
  };
  socket.onmessage = (ev) => {
    try {
      onMessage(JSON.parse(ev.data));
    } catch {
      /* a broken frame is dropped; the next one replaces it */
    }
  };
  socket.onclose = () => {
    live.connected = false;
    retry = setTimeout(connect, 1000);
  };
}

/** Loads the status, the mix and the configuration, then follows the WebSocket; polls every 2 s while it is down. */
export async function startLive() {
  await refreshStatus();
  refreshConfig();
  connect();
  timer = setInterval(async () => {
    if (live.connected) return;
    await refreshStatus();
    try {
      live.mix = await api.get("/api/v1/mix");
      live.meters = await api.get("/api/v1/meters");
    } catch {
      /* the banner tells */
    }
  }, 2000);
}

export function stopLive() {
  clearInterval(timer);
  clearTimeout(retry);
  if (socket) {
    socket.onclose = null;
    socket.close();
  }
}
