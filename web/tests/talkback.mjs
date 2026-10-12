// Browser talkback in headless Chrome (§10.2): the page registers its microphone with the mixer
// (POST /api/v1/talkback/browser, repeated), publishes it by WHIP to MediaMTX, as the UI does. The
// microphone is synthetic: Web Audio plays a 1 kHz burst (20 ms, 0.5) at the start of every wall-clock
// second into a MediaStream, so the mixer's output shows the latency (tests/integration/talkback.sh
// reads it with mxl-mixer-probe --onsets). A burst's time is the moment Web Audio renders it, as a
// microphone's sample is captured.
//
// node web/tests/talkback.mjs <mixer base> <client id> <seconds>
// Prints one JSON line: {id, path, whip_status, connected, ice}.
import { chromium } from "playwright-core";

const [mixer, client, seconds] = process.argv.slice(2);
const browser = await chromium.launch({
  executablePath: process.env.CHROME || "/usr/bin/google-chrome",
  args: ["--no-sandbox", "--autoplay-policy=no-user-gesture-required"],
});
try {
  const page = await browser.newPage();
  // WHIP answers 4xx until the registration has reached MediaMTX's configuration; that is retried.
  page.on("console", (m) => m.type() === "error" && !/40[04]/.test(m.text()) && console.error(`[page] ${m.text()}`));
  await page.goto(`${mixer}/livez`);
  const result = await page.evaluate(
    async ({ client, seconds }) => {
      const register = async () => {
        const r = await fetch("/api/v1/talkback/browser", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ client }) });
        if (!r.ok) throw new Error(`register: ${r.status} ${await r.text()}`);
        return r.json();
      };
      const reg = await register();
      const heartbeat = setInterval(() => register().catch(() => {}), 5000);
      const whip = reg.whip_url || `${location.protocol}//${location.hostname}:${reg.whip_port}/${reg.path}/whip`;

      const ctx = new AudioContext({ sampleRate: 48000 });
      await ctx.resume();
      const dest = ctx.createMediaStreamDestination();
      dest.channelCount = 1;
      // A burst at the start of each wall-clock second, scheduled 1.5 s ahead.
      const wallToCtx = (wall) => ctx.currentTime + (wall - Date.now()) / 1000;
      let next = Math.ceil(Date.now() / 1000) * 1000 + 1000;
      const schedule = () => {
        while (next < Date.now() + 1500) {
          const t = wallToCtx(next);
          const osc = ctx.createOscillator();
          osc.frequency.value = 1000;
          const g = ctx.createGain();
          g.gain.value = 0.5;
          osc.connect(g).connect(dest);
          osc.start(t);
          osc.stop(t + 0.02);
          next += 1000;
        }
      };
      schedule();
      const scheduler = setInterval(schedule, 250);

      const pc = new RTCPeerConnection();
      pc.addTransceiver(dest.stream.getAudioTracks()[0], { direction: "sendonly", streams: [dest.stream] });
      await pc.setLocalDescription(await pc.createOffer());
      await new Promise((resolve) => {
        if (pc.iceGatheringState === "complete") return resolve();
        pc.onicegatheringstatechange = () => pc.iceGatheringState === "complete" && resolve();
        setTimeout(resolve, 2000);
      });
      let status = 0;
      let answer;
      for (let i = 0; i < 20; i++) {
        const r = await fetch(whip, { method: "POST", headers: { "content-type": "application/sdp" }, body: pc.localDescription.sdp });
        status = r.status;
        if (r.status === 201) {
          answer = await r.text();
          break;
        }
        await new Promise((s) => setTimeout(s, 250));
      }
      if (!answer) throw new Error(`WHIP ${status}`);
      await pc.setRemoteDescription({ type: "answer", sdp: answer });
      const connected = await new Promise((resolve) => {
        const check = () => pc.connectionState === "connected" && resolve(true);
        pc.onconnectionstatechange = check;
        check();
        setTimeout(() => resolve(pc.connectionState === "connected"), 5000);
      });
      await new Promise((s) => setTimeout(s, seconds * 1000));
      const stats = await pc.getStats();
      let ice = null;
      stats.forEach((s) => {
        if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") ice = { rtt_ms: (s.currentRoundTripTime || 0) * 1000 };
      });
      clearInterval(scheduler);
      clearInterval(heartbeat);
      pc.close();
      await ctx.close();
      return { id: reg.id, path: reg.path, whip_status: status, connected, ice };
    },
    { client, seconds: Number(seconds) },
  );
  console.log(JSON.stringify(result));
} finally {
  await browser.close();
}
