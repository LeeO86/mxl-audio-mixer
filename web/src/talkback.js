// Talkback in the browser (§10.2): talk presses, and this browser's microphone.
// - Talk: PUT /api/v1/talkback/talk/<mic>/<destination>; a momentary press repeats it every second while held
//   (the mixer ends a momentary talk not repeated within 3 s), its release DELETEs it.
// - The microphone: registered with POST /api/v1/talkback/browser {client} (repeated every 5 s while on), then
//   published by WHIP to MediaMTX (`<PREVIEW_WHEP_URL>/<path>/whip`, or the page's host and the WHEP port of the
//   built-in MediaMTX). The mixer reads it there as the mic `browser-<n>`. Browsers give the microphone only to
//   HTTPS pages and localhost.
import { reactive } from "vue";
import { api } from "./api.js";
import { live } from "./store.js";

const CLIENT_KEY = "mxl-audio-mixer.talkback-client";

export function clientId() {
  let id = sessionStorage.getItem(CLIENT_KEY);
  if (!id) {
    id = `c${Math.random().toString(16).slice(2, 10)}`;
    sessionStorage.setItem(CLIENT_KEY, id);
  }
  return id;
}

/** This browser's microphone: state "off" | "starting" | "on" | "error", the mic id once registered. */
export const browserMic = reactive({ state: "off", id: null, error: "" });
const mic = { stream: null, pc: null, beat: 0, retry: 0, location: null, url: null, stopped: true };

export const micAvailable = () => window.isSecureContext && !!navigator.mediaDevices?.getUserMedia;

async function register() {
  const r = await api.post("/api/v1/talkback/browser", { client: clientId() });
  browserMic.id = r.id;
  return r;
}

async function publish() {
  if (mic.stopped) return;
  try {
    const reg = await register();
    mic.url = reg.whip_url || `${location.protocol}//${location.hostname}:${reg.whip_port}/${reg.path}/whip`;
    const pc = new RTCPeerConnection();
    mic.pc = pc;
    for (const t of mic.stream.getAudioTracks()) pc.addTransceiver(t, { direction: "sendonly", streams: [mic.stream] });
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "connected") browserMic.state = "on";
      if (["failed", "disconnected", "closed"].includes(pc.connectionState) && !mic.stopped && mic.pc === pc) retry("connection lost");
    };
    await pc.setLocalDescription(await pc.createOffer());
    await new Promise((resolve) => {
      if (pc.iceGatheringState === "complete") return resolve();
      pc.addEventListener("icegatheringstatechange", () => pc.iceGatheringState === "complete" && resolve());
      setTimeout(resolve, 1500);
    });
    const resp = await fetch(mic.url, { method: "POST", headers: { "Content-Type": "application/sdp" }, body: pc.localDescription.sdp });
    if (resp.status !== 201) throw new Error(`WHIP ${resp.status}`);
    mic.location = resp.headers.get("Location");
    await pc.setRemoteDescription({ type: "answer", sdp: await resp.text() });
  } catch (e) {
    retry(e.message);
  }
}

function retry(why) {
  if (mic.stopped) return;
  browserMic.state = "error";
  browserMic.error = why;
  mic.pc?.close();
  mic.pc = null;
  clearTimeout(mic.retry);
  mic.retry = setTimeout(publish, 2000);
}

/** Switch this browser's microphone on: ask for it, register it, publish it. */
export async function micOn() {
  if (!mic.stopped) return;
  mic.stopped = false;
  browserMic.state = "starting";
  browserMic.error = "";
  try {
    mic.stream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: false, channelCount: 1 },
    });
  } catch (e) {
    mic.stopped = true;
    browserMic.state = "error";
    browserMic.error = `microphone: ${e.message}`;
    return;
  }
  mic.beat = setInterval(() => register().catch(() => {}), 5000);
  publish();
}

export function micOff() {
  mic.stopped = true;
  clearTimeout(mic.retry);
  clearInterval(mic.beat);
  if (mic.location && mic.url) fetch(new URL(mic.location, mic.url).toString(), { method: "DELETE" }).catch(() => {});
  mic.pc?.close();
  mic.pc = null;
  for (const t of mic.stream?.getTracks() || []) t.stop();
  mic.stream = null;
  if (browserMic.id) api.del(`/api/v1/talkback/browser/${encodeURIComponent(browserMic.id)}`).catch(() => {});
  browserMic.state = "off";
  browserMic.id = null;
}
window.addEventListener("pagehide", () => !mic.stopped && micOff());

// ---- talk ----------------------------------------------------------------------------

/** Who talks to what (from the status). */
export const talking = () => live.status?.talkback?.talking || {};
export const isTalking = (micId, dest) => (talking()[micId] || []).includes(dest);

const held = new Map(); // "mic dest" -> interval
const path = (micId, dest) => `/api/v1/talkback/talk/${encodeURIComponent(micId)}/${encodeURIComponent(dest)}`;

function show(micId, dest, on) {
  if (!live.status) return;
  const t = { ...(live.status.talkback || {}) };
  const now = { ...(t.talking || {}) };
  const set = new Set(now[micId] || []);
  if (on) set.add(dest);
  else set.delete(dest);
  if (set.size) now[micId] = [...set];
  else delete now[micId];
  t.talking = now;
  live.status.talkback = t;
}

/** Start talking to `dest`: latched, or momentary (repeated while held). */
export function talkOn(micId, dest, momentary) {
  const key = `${micId} ${dest}`;
  clearInterval(held.get(key));
  held.delete(key);
  show(micId, dest, true);
  const put = () => api.put(path(micId, dest), { momentary }).catch((e) => (live.actionError = e.message));
  put();
  if (momentary) held.set(key, setInterval(put, 1000));
}

export function talkOff(micId, dest) {
  const key = `${micId} ${dest}`;
  clearInterval(held.get(key));
  held.delete(key);
  show(micId, dest, false);
  api.del(path(micId, dest)).catch((e) => (live.actionError = e.message));
}

/** Pointer handling of a talk button: a short press latches (or releases a latch), a long one is momentary. */
export function talkButton(micId, dest, longMs = 400) {
  let down = 0;
  let wasOn = false;
  return {
    down() {
      down = performance.now();
      wasOn = isTalking(micId, dest) && !held.has(`${micId} ${dest}`);
      if (!wasOn) talkOn(micId, dest, true);
    },
    up() {
      if (!down) return;
      const long = performance.now() - down >= longMs;
      down = 0;
      if (wasOn) talkOff(micId, dest);
      else if (long) talkOff(micId, dest);
      else talkOn(micId, dest, false);
    },
  };
}

export const listenBack = () => live.status?.talkback?.listen_back || null;
