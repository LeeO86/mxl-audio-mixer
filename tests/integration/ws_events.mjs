// Follows /api/v1/events for a while (Node 22: global WebSocket and fetch):
// subscribes to one signal's meters and the analyser, changes that signal's fader
// through REST half-way, and prints what arrived as JSON.
//
// node ws_events.mjs <http base> <signal> <seconds> <fader dB>
const [base, signal, seconds, fader] = process.argv.slice(2);
const url = base.replace(/^http/, "ws") + "/api/v1/events";
const out = { status: 0, version: null, mixes: [], meters: 0, meterSignals: [], analysis: 0, analysisSignal: null };

const ws = new WebSocket(url);
ws.onopen = () => ws.send(JSON.stringify({ type: "subscribe", meters: [signal], analysis: true }));
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.type === "status") {
    out.status += 1;
    out.version = m.status.version;
  } else if (m.type === "mix") {
    const s = m.mix.strips.find((x) => x.id === signal);
    out.mixes.push({ revision: m.revision, fader_db: s ? s.fader_db : null });
  } else if (m.type === "meters") {
    out.meters += 1;
    out.meterSignals = Object.keys(m.meters);
  } else if (m.type === "analysis") {
    out.analysis += 1;
    out.analysisSignal = m.signal;
  }
};
ws.onerror = (e) => {
  console.error("websocket error", e.message);
  process.exit(1);
};

setTimeout(async () => {
  const r = await fetch(`${base}/api/v1/strips/${signal}`, {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ fader_db: Number(fader) }),
  });
  if (!r.ok) {
    console.error("PATCH", r.status, await r.text());
    process.exit(1);
  }
}, (Number(seconds) * 1000) / 2);

setTimeout(() => {
  ws.close();
  console.log(JSON.stringify(out));
  process.exit(0);
}, Number(seconds) * 1000);
