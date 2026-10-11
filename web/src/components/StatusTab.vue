<script setup>
// Status (§15): the probes, versions, timing, the DSP thread and the counters of /metrics.
import { computed, onMounted, onUnmounted, ref } from "vue";
import Pill from "./Pill.vue";
import { api, parseMetrics, probe } from "../api.js";
import { live } from "../store.js";

const probes = ref({ livez: null, readyz: null });
const metrics = ref({});
let timer = 0;
async function load() {
  const [livez, readyz] = await Promise.all([probe("/livez"), probe("/readyz")]);
  probes.value = { livez, readyz };
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
</template>
