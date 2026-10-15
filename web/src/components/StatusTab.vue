<script setup>
// Status (§15): the probes, versions, timing, the DSP thread, the preview (Appendix B.1: mode, the
// built-in MediaMTX, every stream) and the counters of /metrics.
import { computed, onMounted, onUnmounted, ref } from "vue";
import Pill from "./Pill.vue";
import { api, parseMetrics, probe } from "../api.js";
import { live } from "../store.js";

const probes = ref({ livez: null, readyz: null });
const metrics = ref({});
const preview = ref(null);
let timer = 0;
async function load() {
  const [livez, readyz] = await Promise.all([probe("/livez"), probe("/readyz")]);
  probes.value = { livez, readyz };
  preview.value = await api.get("/api/v1/preview").catch(() => preview.value);
  try {
    metrics.value = parseMetrics(await api.text("/metrics"));
  } catch {
    /* keep the last values */
  }
}
onMounted(() => {
  load();
  timer = setInterval(load, 2000);
});
onUnmounted(() => clearInterval(timer));

const code = (status) => ({ text: status ? String(status) : "no answer", kind: status === 200 ? "ok" : status ? "warn" : "bad" });
const s = computed(() => live.status || {});
const ready = computed(() => s.value.readiness || {});
const dsp = computed(() => s.value.dsp || {});
const metric = (name) => metrics.value[name]?.[0]?.value;
const steps = computed(() => (metrics.value.nodes || []).map((m) => [m.labels.type, m.value]));
const blockMean = computed(() => {
  const sum = metric("dsp_block_seconds_sum");
  const n = metric("dsp_block_seconds_count");
  return n ? ((sum / n) * 1e6).toFixed(0) : "–";
});
const streamKind = (state) => ({ publishing: "ok", connecting: "warn" })[state] || "";
const uptime = computed(() => {
  const t = s.value.uptime_s || 0;
  return `${Math.floor(t / 3600)} h ${Math.floor((t % 3600) / 60)} min ${t % 60} s`;
});
</script>

<template>
  <div class="grid two">
    <div class="panel">
      <h3>Health</h3>
      <dl class="kv">
        <dt>/livez</dt>
        <dd><Pill v-bind="code(probes.livez)" /></dd>
        <dt>/readyz</dt>
        <dd><Pill v-bind="code(probes.readyz)" /> <span class="muted small">200 when the DSP runs, timing is trusted and, with a registry, registered</span></dd>
        <dt>Timing</dt>
        <dd><Pill :text="ready.timing_trusted ? `trusted (TAI − UTC ${ready.tai_utc_offset_s} s)` : 'untrusted'" :kind="ready.timing_trusted ? 'ok' : 'bad'" /></dd>
        <dt>Live updates</dt>
        <dd><Pill :text="live.connected ? 'connected' : 'reconnecting'" :kind="live.connected ? 'ok' : 'warn'" /></dd>
        <dt>Remote link</dt>
        <dd>
          <template v-if="s.link">
            {{ s.link.role }}
            <Pill v-if="s.link.role === 'slave'" :text="s.link.up ? 'master linked' : s.link.seen ? 'master lost' : 'waiting'" :kind="s.link.up ? 'ok' : s.link.seen ? 'warn' : 'neutral'" />
          </template>
          <template v-else>–</template>
        </dd>
        <dt>Uptime</dt>
        <dd>{{ uptime }}</dd>
      </dl>
    </div>
    <div class="panel">
      <h3>Versions</h3>
      <dl class="kv">
        <dt>Mixer</dt>
        <dd>{{ s.version || "–" }}</dd>
        <dt>MXL</dt>
        <dd><code>{{ s.mxl_revision || "–" }}</code></dd>
        <dt>Label</dt>
        <dd>{{ s.label || "–" }}</dd>
      </dl>
    </div>
  </div>
  <div class="grid two">
    <div class="panel">
      <h3>DSP</h3>
      <dl class="kv">
        <dt>Block</dt>
        <dd>{{ dsp.block_samples }} samples ({{ (dsp.block_samples / 48).toFixed(1) }} ms), read offset {{ dsp.read_offset_ms }} ms</dd>
        <dt>Load</dt>
        <dd>{{ dsp.load_ratio !== undefined ? `${(dsp.load_ratio * 100).toFixed(1)} %` : "–" }} (last block), mean block time {{ blockMean }} µs</dd>
        <dt>Blocks</dt>
        <dd>{{ dsp.blocks }}</dd>
        <dt>Deadline misses</dt>
        <dd><Pill :text="String(dsp.deadline_misses ?? '–')" :kind="dsp.deadline_misses ? 'warn' : 'ok'" /></dd>
        <dt>Plan swaps</dt>
        <dd>{{ dsp.plan_swaps }}</dd>
        <dt>Scheduling</dt>
        <dd>{{ dsp.scheduling || "–" }}</dd>
      </dl>
    </div>
    <div class="panel">
      <h3>Plan</h3>
      <table>
        <thead><tr><th>Step</th><th class="num">Count</th></tr></thead>
        <tbody>
          <tr v-for="[t, n] in steps" :key="t"><td>{{ t }}</td><td class="num">{{ n }}</td></tr>
        </tbody>
      </table>
      <p class="note">All counters and histograms: <a href="/metrics" target="_blank">/metrics</a> (Prometheus, prefix <code>mxl_audio_mixer_</code>).</p>
    </div>
  </div>
  <div v-if="preview" class="panel preview-status">
    <h3>Preview (monitors in the browser)</h3>
    <dl class="kv">
      <dt>Mode</dt>
      <dd>
        <template v-if="preview.mode === 'own'">
          built-in MediaMTX
          <Pill :text="preview.child?.running ? `running, pid ${preview.child.pid}` : 'not running'" :kind="preview.child?.running ? 'ok' : 'bad'" />
          <span class="muted small">restarts {{ preview.child?.restarts ?? 0 }}<template v-if="preview.child?.last_exit">, last exit: {{ preview.child.last_exit }}</template>; WHEP on port {{ preview.whep_port }}</span>
        </template>
        <template v-else>shared MediaMTX <span class="muted small">{{ preview.whep_url || "" }}</span></template>
      </dd>
    </dl>
    <table>
      <thead><tr><th>Stream</th><th>State</th><th class="num">Frames</th><th class="num">Connects</th><th class="num">Readers</th><th>Error</th></tr></thead>
      <tbody>
        <tr v-for="x in preview.streams" :key="x.id" :data-stream="x.id">
          <td><code>{{ x.path }}</code></td>
          <td><Pill :text="x.state" :kind="streamKind(x.state)" /></td>
          <td class="num">{{ x.frames }}</td>
          <td class="num">{{ x.connects }}</td>
          <td class="num">{{ x.readers ?? "–" }}</td>
          <td class="small">{{ x.error }}</td>
        </tr>
        <tr v-if="!preview.streams.length"><td colspan="6" class="muted">no monitors</td></tr>
      </tbody>
    </table>
    <p class="note">A stream is encoded only while someone listens; it stops 10 s after the last listener.</p>
  </div>
</template>
