// Shared UI state: the status, the mix and the meters from the WebSocket /api/v1/events (README "Live
// updates"), the configuration, and the actions every view uses. Every client sees every change: the
// mix comes back on the WebSocket after a change from any client.
import { computed, reactive } from "vue";
import { api } from "./api.js";

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
// a fast second control does not drop the first.

const pending = new Map(); // "METHOD path" -> body
const timers = new Map(); // key -> timeout
const lastSend = new Map(); // key -> time

function flushKey(key) {
  clearTimeout(timers.get(key));
  timers.delete(key);
  const body = pending.get(key);
  pending.delete(key);
  if (!body) return;
  lastSend.set(key, performance.now());
  const [method, path] = key.split(" ");
  act(() => (method === "PUT" ? api.put(path, body) : api.patch(path, body))).then((ok) => {
    // Refused: the server's mix did not change, so take it back over the local change.
    if (ok === undefined) refreshMix();
  });
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
  const wait = 33 - (performance.now() - (lastSend.get(key) || 0));
  if (wait <= 0) flushKey(key);
  else if (!timers.has(key)) timers.set(key, setTimeout(() => flushKey(key), wait));
}

// Changes show at once in this client (the next mix from the server replaces them), so a toggle pressed
// twice quickly toggles twice.
function local(target, body) {
  if (!target) return;
  for (const [k, v] of Object.entries(body)) if (k !== "ramp_ms") target[k] = v;
}
export const signalPath = (id) => `/api/v1/${isBus(id) ? "buses" : "strips"}/${encodeURIComponent(id)}`;
export function patchSignal(id, body) {
  local(signalOf(id), body);
  patch(signalPath(id), body);
}
export function patchSend(from, to, body) {
  local(sendOf(from, to), body);
  patch(`/api/v1/sends/${encodeURIComponent(from)}/${encodeURIComponent(to)}`, body, "PUT");
}
export function patchNode(id, node, body) {
  local(signalOf(id)?.chain?.find((n) => n.id === node), body);
  patch(`${signalPath(id)}/chain/${encodeURIComponent(node)}`, body);
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
  local(room.value.monitors.find((m) => m.id === id), body);
  patch(`/api/v1/monitors/${encodeURIComponent(id)}`, body);
};

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
    live.mix = m.mix;
    live.revision = m.revision;
    live.latency = m.latency_ms || {};
  } else if (m.type === "meters") {
    live.meters = m.meters;
    live.loudness = m.loudness;
  } else if (m.type === "analysis") live.analysis = m;
}

function connect() {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  socket = new WebSocket(`${proto}//${location.host}/api/v1/events`);
  socket.onopen = () => {
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
