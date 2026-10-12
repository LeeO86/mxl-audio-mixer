// Listening to a monitor in the browser (§10.1, Appendix B.1): ask the mixer to stream it
// (POST /api/v1/preview/<id>/listen, repeated every 5 s while playing), then play it by WHEP from
// MediaMTX: `<PREVIEW_WHEP_URL>/<path>/whep`, or with the built-in MediaMTX the page's host and its
// WHEP port. Reconnects by itself about every 2 s, as the sibling functions do.
import { reactive } from "vue";
import { api } from "./api.js";

export const listening = reactive({}); // monitor id -> "connecting" | "playing" | "error: …"

const players = new Map(); // id -> { pc, audio, timer, beat, location, stopped }

function whepUrl(answer) {
  if (answer.whep_url) return answer.whep_url;
  return `${location.protocol}//${location.hostname}:${answer.whep_port}/${answer.path}/whep`;
}

async function iceGathered(pc) {
  if (pc.iceGatheringState === "complete") return;
  await new Promise((resolve) => {
    const done = () => pc.iceGatheringState === "complete" && resolve();
    pc.addEventListener("icegatheringstatechange", done);
    setTimeout(resolve, 1500);
  });
}

async function connect(id) {
  const p = players.get(id);
  if (!p || p.stopped) return;
  listening[id] = "connecting";
  try {
    const answer = await api.post(`/api/v1/preview/${encodeURIComponent(id)}/listen`);
    const pc = new RTCPeerConnection();
    p.pc = pc;
    pc.addTransceiver("audio", { direction: "recvonly" });
    pc.ontrack = (e) => {
      p.audio.srcObject = e.streams[0] || new MediaStream([e.track]);
      p.audio.play().catch(() => {});
      p.stream = p.audio.srcObject;
    };
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "connected") listening[id] = "playing";
      if (["failed", "disconnected", "closed"].includes(pc.connectionState) && !p.stopped) retry(id, "connection lost");
    };
    await pc.setLocalDescription(await pc.createOffer());
    await iceGathered(pc);
    // The stream needs a moment to be published after the first listen request.
    let resp;
    p.url = whepUrl(answer);
    for (let attempt = 0; attempt < 10; attempt++) {
      resp = await fetch(p.url, { method: "POST", headers: { "Content-Type": "application/sdp" }, body: pc.localDescription.sdp });
      if (resp.status !== 404) break;
      await new Promise((r) => setTimeout(r, 300));
    }
    if (!resp.ok) throw new Error(`WHEP ${resp.status}`);
    p.location = resp.headers.get("Location");
    await pc.setRemoteDescription({ type: "answer", sdp: await resp.text() });
  } catch (e) {
    retry(id, e.message);
  }
}

function retry(id, why) {
  const p = players.get(id);
  if (!p || p.stopped) return;
  listening[id] = `error: ${why}`;
  p.pc?.close();
  p.pc = null;
  clearTimeout(p.timer);
  p.timer = setTimeout(() => connect(id), 2000);
}

/** Start listening to monitor (or personal monitor) `id`. */
export function listen(id) {
  if (players.has(id)) return players.get(id);
  const audio = new Audio();
  audio.autoplay = true;
  const p = { pc: null, audio, timer: 0, beat: 0, location: null, url: null, stopped: false, stream: null };
  players.set(id, p);
  p.beat = setInterval(() => api.post(`/api/v1/preview/${encodeURIComponent(id)}/listen`).catch(() => {}), 5000);
  connect(id);
  return p;
}

/** Stop listening to `id`. */
export function stopListening(id) {
  const p = players.get(id);
  if (!p) return;
  p.stopped = true;
  clearTimeout(p.timer);
  clearInterval(p.beat);
  if (p.location) fetch(new URL(p.location, p.url).toString(), { method: "DELETE" }).catch(() => {});
  p.pc?.close();
  p.audio.srcObject = null;
  players.delete(id);
  delete listening[id];
}

export function toggleListen(id) {
  if (players.has(id)) stopListening(id);
  else listen(id);
}
