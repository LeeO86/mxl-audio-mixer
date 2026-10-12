// A monitor in headless Chrome over WHEP (Appendix B.1), as the UI plays it: the listen request
// (repeated), the WHEP offer to MediaMTX, the track into Web Audio. With the `second-pulse`
// oscillator on the monitor (a 1 kHz burst in the first 20 ms of every TAI second) it measures
// the latency: the wall-clock time at which each burst's onset reaches the audio output
// (AudioContext.getOutputTimestamp), modulo one second. TAI and UTC seconds differ by whole
// seconds, and the mixer and the browser share the clock.
//
// node web/tests/preview.mjs <mixer base> <monitor id (a personal one too)> <seconds> [once]
// With `once`, a single listen request: only MediaMTX's reader count can keep the stream longer.
// Prints one JSON line: {received_s, peak, onsets, latency_ms: {min, median, max}, ice}.
import { chromium } from "playwright-core";

const [mixer, monitor, seconds, once] = process.argv.slice(2);
const browser = await chromium.launch({
  executablePath: process.env.CHROME || "/usr/bin/google-chrome",
  // The measuring AudioWorklet needs a secure context; the UI's player (RTCPeerConnection, <audio>) does not.
  args: ["--no-sandbox", "--autoplay-policy=no-user-gesture-required", `--unsafely-treat-insecure-origin-as-secure=${new URL(mixer).origin}`],
});
try {
  const page = await browser.newPage();
  // WHEP answers 404 until the encoder has published the path; that is retried.
  page.on("console", (m) => m.type() === "error" && !m.text().includes("404") && console.error(`[page] ${m.text()}`));
  await page.goto(`${mixer}/livez`);
  const result = await page.evaluate(
    async ({ monitor, seconds, once }) => {
      const listenUrl = `/api/v1/preview/${monitor}/listen`;
      const listen = async () => {
        const r = await fetch(listenUrl, { method: "POST" });
        if (!r.ok) throw new Error(`listen: ${r.status} ${await r.text()}`);
        return r.json();
      };
      const l = await listen();
      const heartbeat = once ? 0 : setInterval(() => listen().catch(() => {}), 5000);
      const whep = l.whep_url || `${location.protocol}//${location.hostname}:${l.whep_port}/${l.path}/whep`;

      const pc = new RTCPeerConnection();
      pc.addTransceiver("audio", { direction: "recvonly" });
      const track = new Promise((resolve) => (pc.ontrack = (e) => resolve(e.streams[0] || new MediaStream([e.track]))));
      await pc.setLocalDescription(await pc.createOffer());
      await new Promise((resolve) => {
        if (pc.iceGatheringState === "complete") return resolve();
        pc.onicegatheringstatechange = () => pc.iceGatheringState === "complete" && resolve();
        setTimeout(resolve, 2000);
      });
      // The encoder may still be connecting to MediaMTX: the path appears within a few seconds.
      let answer;
      for (let i = 0; i < 40; i++) {
        const r = await fetch(whep, { method: "POST", headers: { "content-type": "application/sdp" }, body: pc.localDescription.sdp });
        if (r.status === 201) {
          answer = await r.text();
          break;
        }
        if (r.status !== 404) throw new Error(`WHEP ${r.status}: ${await r.text()}`);
        await new Promise((s) => setTimeout(s, 250));
      }
      if (!answer) throw new Error("WHEP: no stream after 10 s");
      await pc.setRemoteDescription({ type: "answer", sdp: answer });
      const stream = await track;

      // Chrome only pulls a remote track into Web Audio while a media element plays it.
      const element = new Audio();
      element.muted = true;
      element.srcObject = stream;
      await element.play();
      const ctx = new AudioContext({ sampleRate: 48000, latencyHint: "interactive" });
      await ctx.resume();
      const worklet = `
        class Onsets extends AudioWorkletProcessor {
          constructor() { super(); this.quiet = 0; this.peak = 0; this.frames = 0; this.loud = 0; }
          process(inputs) {
            const x = inputs[0] && inputs[0][0];
            if (!x) return true;
            for (let i = 0; i < x.length; i++) {
              const a = Math.abs(x[i]);
              if (a > this.peak) this.peak = a;
              if (a > 0.05) {
                this.loud++;
                if (this.quiet > 4800) this.port.postMessage({ onset: (currentFrame + i) / sampleRate });
                this.quiet = 0;
              } else this.quiet++;
            }
            this.frames += x.length;
            if (this.frames >= 48000) {
              this.port.postMessage({ peak: this.peak, loud: this.loud, frames: this.frames });
              this.frames = 0; this.peak = 0; this.loud = 0;
            }
            return true;
          }
        }
        registerProcessor("onsets", Onsets);`;
      await ctx.audioWorklet.addModule(URL.createObjectURL(new Blob([worklet], { type: "text/javascript" })));
      const node = new AudioWorkletNode(ctx, "onsets");
      ctx.createMediaStreamSource(stream).connect(node);
      const sink = ctx.createGain();
      sink.gain.value = 0;
      node.connect(sink).connect(ctx.destination);

      const onsets = [];
      let peak = 0;
      let received = 0;
      node.port.onmessage = (e) => {
        if (e.data.onset !== undefined) {
          const ts = ctx.getOutputTimestamp();
          const wall = performance.timeOrigin + ts.performanceTime + (e.data.onset - ts.contextTime) * 1000;
          onsets.push(wall);
        } else {
          peak = Math.max(peak, e.data.peak);
          if (e.data.loud > 0) received++;
        }
      };
      await new Promise((s) => setTimeout(s, seconds * 1000));
      clearInterval(heartbeat);
      const stats = await pc.getStats();
      let ice = null;
      stats.forEach((s) => {
        if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") ice = { rtt_ms: (s.currentRoundTripTime || 0) * 1000 };
      });
      pc.close();
      await ctx.close();
      // The first onset may be the stream starting in the middle of a burst.
      const latency = onsets.slice(1).map((w) => ((w % 1000) + 1000) % 1000).sort((a, b) => a - b);
      const at = (q) => (latency.length ? Math.round(latency[Math.min(latency.length - 1, Math.floor(q * latency.length))] * 10) / 10 : null);
      return {
        received_s: received,
        peak,
        onsets: onsets.length,
        latency_ms: { min: at(0), median: at(0.5), max: at(0.999) },
        output_latency_ms: Math.round((ctx.outputLatency || 0) * 1000),
        base_latency_ms: Math.round((ctx.baseLatency || 0) * 1000),
        ice,
      };
    },
    { monitor, seconds: Number(seconds), once: once === "once" },
  );
  console.log(JSON.stringify(result));
} finally {
  await browser.close();
}
