<script setup>
// One monitor path (§10.1): the source buttons, level (with the SPL presets of a calibrated monitor), dim,
// cut, the L/R check, solo, its meter; in full, the room alignment and the MXL output too. `compact` is the
// monitor widget: source, level, dim, cut, mono.
import { computed } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import Meter from "./Meter.vue";
import Pot from "./Pot.vue";
import DownmixEdit from "./DownmixEdit.vue";
import Segmented from "./Segmented.vue";
import { api } from "../api.js";
import { listening, toggleListen } from "../whep.js";
import { act, buses, live, patchMonitor, room, signalOf, signals, soloed } from "../store.js";

const props = defineProps({
  monitor: { type: Object, required: true },
  compact: { type: Boolean, default: false },
});
const m = computed(() => props.monitor);
const set = (body) => patchMonitor(m.value.id, body);
const key = computed(() => `monitor:${m.value.id}`);
const channels = computed(() => ({ mono: 1, stereo: 2, "3.0": 3, "5.1": 6, "7.1": 8, "5.1.4": 10, "7.1.4": 12 })[m.value.format] || 2);
const soloActive = computed(() => Object.keys(soloed.value).length > 0);
// A control-room talkback mic talks: the monitor is at its talkback dim.
const talkDim = computed(() => {
  const talking = live.status?.talkback?.talking || {};
  return (room.value.talkback?.mics || []).some((x) => x.dims_control_room && (talking[x.id] || []).length);
});
const listenBack = computed(() => live.status?.talkback?.listen_back);

const sourceKey = (s) => (s ? ("signal" in s ? `s:${s.signal}` : `i:${s.input}`) : "");
const sourceName = (s) => (!s ? "–" : "signal" in s ? signalOf(s.signal)?.name || s.signal : `Input ${s.input}`);
const buttons = computed(() => (room.value.sources.length ? room.value.sources : buses.value.map((b) => ({ signal: b.id }))));
const others = computed(() => [
  ...signals.value.map((x) => ({ value: `s:${x.id}`, label: x.name })),
  ...(live.status?.inputs || []).map((i) => ({ value: `i:${i.input}`, label: `Input ${i.input}` })),
]);
function pick(value) {
  if (!value) return set({ source: null });
  const [t, v] = [value.slice(0, 1), value.slice(2)];
  set({ source: t === "s" ? { signal: v } : { input: Number(v) } });
}
const CHECKS = [
  { value: "stereo", label: "Stereo" },
  { value: "mono", label: "Mono" },
  { value: "difference", label: "L−R", title: "phase check: the difference" },
  { value: "swap", label: "Swap" },
  { value: "left", label: "L" },
  { value: "right", label: "R" },
];
const presets = computed(() => (m.value.calibration_spl ? room.value.reference_spl : []));
const removeMonitor = () => act(() => api.del(`/api/v1/monitors/${m.value.id}`));
const hasLfe = computed(() => ["5.1", "7.1", "5.1.4", "7.1.4"].includes(m.value.format));
// The values when bass management is first switched on (IMPLEMENTATION_PLAN.md, M5a, D67).
const BASS_DEFAULTS = { enabled: true, crossover_hz: 80, lfe_gain_db: 10 };
const bass = computed(() => m.value.bass_management || null);
const setBass = (change) => set({ bass_management: { ...(bass.value || BASS_DEFAULTS), ...change } });
const published = computed(() => (live.status?.outputs || []).find((o) => o.signal === key.value));
</script>

<template>
  <div class="panel monitor">
    <h3>
      {{ m.name }} <span class="muted" style="text-transform: none; letter-spacing: 0">{{ m.kind }} · {{ m.format }}</span>
      <span v-if="soloActive && m.follows_solo" class="state-tag" style="background: #e0c23a; color: #20180a">solo</span>
      <span v-if="talkDim && !compact" class="state-tag" style="background: var(--bad); color: #fff" :title="`dimmed by ${m.talkback_dim_db} dB while a control-room mic talks`">talkback</span>
      <span v-if="listenBack && m.follows_solo" class="state-tag" style="background: var(--warn); color: #20180a" :title="`plays ${sourceName(listenBack)}`">listen-back</span>
      <span class="spacer"></span>
      <span v-if="!compact && published" class="pill ok" :title="`MXL flow ${published.mxl_flow_id}`">MXL out</span>
      <button class="btn small" :class="listening[m.id] ? '' : 'secondary'" :aria-pressed="!!listening[m.id]" :title="listening[m.id] || 'listen in this browser (WebRTC)'" @click="toggleListen(m.id)">
        {{ listening[m.id] === "playing" ? "Listening" : listening[m.id] ? "Connecting…" : "Listen" }}
      </button>
    </h3>
    <div class="mbody">
      <div>
        <div class="srcbuttons" role="group" aria-label="Source">
          <button v-for="s in buttons" :key="sourceKey(s)" class="btn" :class="sourceKey(s) === sourceKey(m.source) ? '' : 'secondary'" :aria-pressed="sourceKey(s) === sourceKey(m.source)" @click="set({ source: s })">
            {{ sourceName(s) }}
          </button>
          <select v-if="!compact" :value="sourceKey(m.source)" aria-label="Other source" style="width: auto" @change="pick($event.target.value)">
            <option value="">no source</option>
            <option v-for="o in others" :key="o.value" :value="o.value">{{ o.label }}</option>
          </select>
        </div>
        <Pot label="Level" unit="dB" :value="m.level_db" :min="-80" :max="12" :def="0" :fine="0.5" @change="(v) => set({ level_db: v })" />
        <div v-if="presets.length" class="mbuttons">
          <button v-for="p in presets" :key="p" class="btn small secondary" :title="`level ${(p - m.calibration_spl).toFixed(1)} dB`" @click="set({ level_db: p - m.calibration_spl })">{{ p }} dB SPL</button>
        </div>
        <div class="mbuttons">
          <button class="btn secondary on-warn" :aria-pressed="m.dim" @click="set({ dim: !m.dim })">Dim</button>
          <button class="btn secondary on-bad" :aria-pressed="m.cut" @click="set({ cut: !m.cut })">Cut</button>
          <button v-if="compact" class="btn secondary on-warn" :aria-pressed="m.check === 'mono'" @click="set({ check: m.check === 'mono' ? 'stereo' : 'mono' })">Mono</button>
        </div>
        <template v-if="!compact">
          <label>Check</label>
          <Segmented :model-value="m.check" :options="CHECKS" label="Check" @update:model-value="(v) => set({ check: v })" />
          <label class="check"><input type="checkbox" :checked="m.follows_solo" @change="set({ follows_solo: $event.target.checked })" /> Follows solo (PFL/AFL)</label>
          <div v-if="hasLfe" class="bass">
            <button class="btn secondary on-warn" :aria-pressed="!!bass?.enabled" @click="setBass({ enabled: !bass?.enabled })">Bass management</button>
            <template v-if="bass?.enabled">
              <Pot label="Crossover" unit="Hz" :value="bass.crossover_hz" :min="40" :max="250" :def="80" :fine="0.02" :digits="0" log @change="(v) => setBass({ crossover_hz: v })" />
              <Pot label="LFE gain" unit="dB" :value="bass.lfe_gain_db" :min="-20" :max="20" :def="10" :fine="0.5" @change="(v) => setBass({ lfe_gain_db: v })" />
            </template>
          </div>
          <details>
            <summary class="muted small" style="cursor: pointer; margin-top: 0.4rem">Dim depth, alignment, output</summary>
            <Pot label="Dim depth" unit="dB" :value="m.dim_db" :min="-60" :max="0" :def="-21" :fine="1" @change="(v) => set({ dim_db: v })" />
            <Pot label="Talkback dim" unit="dB" :value="m.talkback_dim_db ?? -21" :min="-60" :max="0" :def="-21" :fine="1" @change="(v) => set({ talkback_dim_db: v })" />
            <Pot label="Trim" unit="dB" :value="m.trim_db" :min="-24" :max="24" :def="0" :fine="0.5" @change="(v) => set({ trim_db: v })" />
            <Pot label="Delay" unit="ms" :value="m.delay_ms" :min="0" :max="2000" :def="0" :fine="0.1" :digits="2" @change="(v) => set({ delay_ms: v })" />
            <label>Calibration: dB SPL at a level of 0 dB (for the presets)</label>
            <input type="number" min="40" max="130" step="0.5" :value="m.calibration_spl ?? ''" placeholder="not calibrated" aria-label="Calibration" @change="set({ calibration_spl: $event.target.value === '' ? null : Number($event.target.value) })" />
            <label class="check"><input type="checkbox" :checked="!!m.downmix" aria-label="Own down-mix" @change="set({ downmix: $event.target.checked ? { ...(live.mix?.settings?.downmix || {}) } : null })" /> Own down-mix of its source and solo</label>
            <DownmixEdit v-if="m.downmix" :value="m.downmix" label="Monitor down-mix" @change="(d) => set({ downmix: d })" />
            <p class="note">{{ m.chain.length }} alignment nodes (EQ, filters for bass management): <code>PATCH /api/v1/monitors/{{ m.id }}</code> <code>chain</code>.</p>
            <label class="check"><input type="checkbox" :checked="m.publish" @change="set({ publish: $event.target.checked })" /> Publish as an MXL flow (NMOS sender "Monitor {{ m.name }}")</label>
            <div class="actions"><ConfirmButton label="Delete monitor" small @confirm="removeMonitor" /></div>
          </details>
        </template>
      </div>
      <Meter :signal="key" :values="live.meters[key]" :channels="channels" scale="dbfs" />
    </div>
  </div>
</template>
