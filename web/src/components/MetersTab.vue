<script setup>
// Metering (§7, §12.3): the bridge (every signal on the chosen scale, peak hold, correlation), loudness
// (EBU R 128: momentary, short-term, integrated, range, true peak; start, stop, reset) and the analyser
// (spectrum in 1/6-octave bands and goniometer of one tap point).
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import Meter from "./Meter.vue";
import Pill from "./Pill.vue";
import Segmented from "./Segmented.vue";
import { SCALES } from "../dsp.js";
import { channelLabels } from "../console.js";
import { TAPS } from "../nodes.js";
import { api, fmtHz } from "../api.js";
import { act, live, patchSignal, signalOf, signals, store, stored, wantAnalysis } from "../store.js";

const scale = ref(stored("meter.scale", "dbfs"));
const alignment = ref(Number(stored("meter.alignment", -18)));
watch(scale, (v) => store("meter.scale", v));
watch(alignment, (v) => store("meter.alignment", v));
const ALIGNMENTS = [
  { value: -18, label: "−18 dBFS (EBU R 68)" },
  { value: -20, label: "−20 dBFS (SMPTE RP 155)" },
];
const channels = (s) => ({ mono: 1, stereo: 2, "3.0": 3, "5.1": 6, "7.1": 8, "5.1.4": 10, "7.1.4": 12 })[s.format] || 2;
const corr = (id) => live.meters[id]?.correlation;
const clearAll = () => act(() => api.post("/api/v1/meters/clear"));

// Loudness.
const settings = computed(() => live.mix?.settings || {});
const lufs = (v) => (v === null || v === undefined || v === "-inf" || !Number.isFinite(Number(v)) || Number(v) < -199 ? "−∞" : Number(v).toFixed(1));
const loudSignals = computed(() => signals.value.filter((s) => s.loudness));
const quiet = computed(() => signals.value.filter((s) => !s.loudness));
const loudAct = (id, action) => act(() => api.post(`/api/v1/loudness/${id}/${action}`));

// Analyser.
const target = ref(null);
const pick = ref({ signal: "", tap: "post-fader" });
async function loadTarget() {
  try {
    const a = await api.get("/api/v1/analysis");
    target.value = a.target;
    if (a.target) pick.value = { signal: a.target.signal, tap: typeof a.target.tap === "string" ? a.target.tap : "post-fader" };
    else if (!pick.value.signal && signals.value[0]) pick.value.signal = signals.value[0].id;
  } catch {
    /* the banner tells */
  }
}
async function analyse() {
  const r = await act(() => api.put("/api/v1/analysis", pick.value));
  if (r) target.value = r.target;
}
async function stopAnalysis() {
  if (await act(() => api.del("/api/v1/analysis"))) target.value = null;
}
watch(
  signals,
  (list) => {
    if (!pick.value.signal && list[0]) pick.value.signal = list[0].id;
  },
  { immediate: true },
);
onMounted(() => {
  wantAnalysis(true);
  loadTarget();
});
onUnmounted(() => wantAnalysis(false));

const a = computed(() => live.analysis);
const SW = 600;
const SH = 240;
const sy = (db) => Math.max(0, Math.min(SH - 16, ((db + 90) / 90) * (SH - 16)));
const bars = computed(() => {
  const v = a.value;
  if (!v) return [];
  const n = v.spectrum_db.length;
  const w = SW / n;
  return v.spectrum_db.map((db, i) => ({ x: i * w + 0.5, w: w - 1, h: sy(db), f: v.bands_hz[i] }));
});
const SPEC_LABELS = [31.5, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
const labelX = (f) => {
  const v = a.value;
  if (!v) return 0;
  const i = v.bands_hz.findIndex((b) => Math.abs(Math.log2(b / f)) < 1 / 12);
  return i < 0 ? -100 : (i + 0.5) * (SW / v.bands_hz.length);
};
const G = 240;
const gonio = computed(() => {
  const v = a.value;
  if (!v) return "";
  const peak = Math.max(1e-6, ...v.xy.map(([l, r]) => Math.max(Math.abs(l), Math.abs(r))));
  const k = (G / 2 - 8) / peak / Math.SQRT2;
  return v.xy
    .map(([l, r], i) => {
      const x = G / 2 + (r - l) * k;
      const y = G / 2 - (l + r) * k;
      return `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join("");
});
</script>

<template>
  <div class="toolbar">
    <div class="group">
      <span class="caption">Scale</span>
      <select v-model="scale" aria-label="Meter scale" style="width: auto">
        <option v-for="(s, k) in SCALES" :key="k" :value="k">{{ s.label }}</option>
      </select>
    </div>
    <div class="group">
      <span class="caption">Alignment</span>
      <Segmented v-model="alignment" :options="ALIGNMENTS" label="Alignment level" />
    </div>
    <span class="spacer"></span>
    <button class="btn secondary" @click="clearAll">Clear all peak holds</button>
  </div>

  <div class="panel">
    <h3>Bridge</h3>
    <div class="bridge">
      <div v-for="s in signals" :key="s.id" class="mcol" :style="{ '--colour': s.colour || 'var(--border)' }">
        <span class="mname" :title="s.id">{{ s.name }}</span>
        <Meter :signal="s.id" :values="live.meters[s.id]" :channels="channels(s)" :labels="channelLabels(s.format)" :scale="scale" :alignment="alignment" :muted="!!s.mute" />
        <div v-if="channels(s) >= 2" class="corr" :title="`phase correlation ${corr(s.id)?.toFixed(2) ?? '–'}`">
          <span v-if="corr(s.id) !== undefined && corr(s.id) !== null" :style="{ left: `${((corr(s.id) + 1) / 2) * 100}%` }"></span>
        </div>
      </div>
    </div>
    <p class="note">
      {{ SCALES[scale].label }}. PPM scales sit on the alignment level: EBU and Nordic TEST and BBC 4 at alignment, DIN 0 dB at alignment + 9 dB. The bar under a stereo signal is its
      phase correlation (−1 left, +1 right). Click a peak-hold value to clear it.
    </p>
  </div>

  <div class="panel loud">
    <h3>Loudness <span class="muted" style="text-transform: none; letter-spacing: 0">target {{ settings.loudness_target_lufs }} LUFS ± {{ settings.loudness_tolerance_lu }} LU</span></h3>
    <table>
      <thead>
        <tr><th>Signal</th><th class="num">Momentary</th><th class="num">Short-term</th><th class="num">Integrated</th><th class="num">Range LU</th><th class="num">Max true peak</th><th>State</th><th></th></tr>
      </thead>
      <tbody>
        <tr v-for="s in loudSignals" :key="s.id">
          <td><strong>{{ s.name }}</strong></td>
          <td class="num">{{ lufs(live.loudness[s.id]?.momentary_lufs) }}</td>
          <td class="num">{{ lufs(live.loudness[s.id]?.short_term_lufs) }}</td>
          <td class="num"><span class="big" :class="live.loudness[s.id]?.in_tolerance ? 'ok' : 'warn'">{{ lufs(live.loudness[s.id]?.integrated_lufs) }}</span></td>
          <td class="num">{{ lufs(live.loudness[s.id]?.range_lu) }}</td>
          <td class="num">{{ lufs(live.loudness[s.id]?.max_true_peak_dbtp) }} dBTP</td>
          <td>
            <Pill :text="live.loudness[s.id]?.running ? 'measuring' : 'stopped'" :kind="live.loudness[s.id]?.running ? 'ok' : 'neutral'" />
            <Pill v-if="live.loudness[s.id]?.running" :text="live.loudness[s.id]?.in_tolerance ? 'in tolerance' : 'off target'" :kind="live.loudness[s.id]?.in_tolerance ? 'ok' : 'warn'" />
          </td>
          <td style="text-align: right; white-space: nowrap">
            <button v-if="live.loudness[s.id]?.running" class="btn small secondary" @click="loudAct(s.id, 'stop')">Stop</button>
            <button v-else class="btn small secondary" @click="loudAct(s.id, 'start')">Start</button>
            <button class="btn small secondary" @click="loudAct(s.id, 'reset')">Reset</button>
            <button class="btn small secondary" @click="patchSignal(s.id, { loudness: false })">Remove</button>
          </td>
        </tr>
        <tr v-if="!loudSignals.length"><td colspan="8" class="empty">No signal measures loudness.</td></tr>
      </tbody>
    </table>
    <div v-if="quiet.length" class="row tight" style="margin-top: 0.6rem">
      <select aria-label="Measure loudness of" :value="''" style="width: auto" @change="patchSignal($event.target.value, { loudness: true }), ($event.target.value = '')">
        <option value="" disabled>+ Measure loudness of…</option>
        <option v-for="s in quiet" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
    </div>
  </div>

  <div class="panel">
    <h3>Analyser</h3>
    <div class="row tight">
      <select v-model="pick.signal" aria-label="Signal to analyse" style="width: auto">
        <option v-for="s in signals" :key="s.id" :value="s.id">{{ s.name }}</option>
      </select>
      <select v-model="pick.tap" aria-label="Tap" style="width: auto">
        <option v-for="[v, l] in TAPS" :key="v" :value="v">{{ l }}</option>
      </select>
      <button class="btn" :disabled="!pick.signal" @click="analyse">Analyse</button>
      <button class="btn secondary" :disabled="!target" @click="stopAnalysis">Stop</button>
      <span v-if="target" class="muted small">{{ signalOf(target.signal)?.name || target.signal }} · {{ typeof target.tap === "string" ? target.tap : `after ${target.tap.after}` }}</span>
    </div>
    <div v-if="target && a" class="grid two" style="margin-top: 0.6rem; grid-template-columns: minmax(0, 2fr) minmax(0, 1fr)">
      <svg class="spectrum" :viewBox="`0 0 ${SW} ${SH}`" preserveAspectRatio="none" role="img" aria-label="spectrum">
        <line v-for="d in [-12, -24, -36, -48, -60, -72]" :key="d" x1="0" :x2="SW" :y1="SH - 16 - sy(d)" :y2="SH - 16 - sy(d)" />
        <rect v-for="(b, i) in bars" :key="i" :x="b.x" :width="b.w" :y="SH - 16 - b.h" :height="b.h"><title>{{ fmtHz(b.f) }} Hz</title></rect>
        <text v-for="f in SPEC_LABELS" :key="f" :x="labelX(f)" :y="SH - 3" text-anchor="middle">{{ fmtHz(f) }}</text>
        <text v-for="d in [-12, -36, -60]" :key="`l${d}`" x="2" :y="SH - 18 - sy(d)">{{ d }}</text>
      </svg>
      <div>
        <svg class="gonio" :viewBox="`0 0 ${G} ${G}`" role="img" aria-label="goniometer">
          <line :x1="G / 2" y1="0" :x2="G / 2" :y2="G" />
          <line x1="0" :y1="G / 2" :x2="G" :y2="G / 2" />
          <text :x="G / 2 + 3" y="12">M</text>
          <text x="4" y="12">L</text>
          <text :x="G - 12" y="12">R</text>
          <path :d="gonio" />
        </svg>
        <p class="note" style="text-align: center">Correlation {{ a.correlation?.toFixed(2) ?? "–" }}</p>
      </div>
    </div>
    <p v-else class="note">Pick a signal and a tap point. The spectrum (4096-point FFT, 1/6-octave bands, dBFS) and the goniometer follow it at 25 Hz.</p>
  </div>
</template>
