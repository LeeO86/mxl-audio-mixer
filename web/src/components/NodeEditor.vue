<script setup>
// The parameters of one processing node (§12.2): the curve of its type (EQ response with handles,
// dynamics transfer curve), its fields, and the bands of an EQ, dynamic EQ or multiband: one band at a
// time, picked by its number (or its handle), so the tile stays compact. Changes PATCH the node
// (rate-limited); band lists go as a whole.
import { computed, ref } from "vue";
import DynCurve from "./DynCurve.vue";
import EqCurve from "./EqCurve.vue";
import Pot from "./Pot.vue";
import Segmented from "./Segmented.vue";
import SurroundPad from "./SurroundPad.vue";
import { BAND_TYPES, NODE_TYPES, PASS_BANDS, SLOPES, TAPS } from "../nodes.js";
import { live, patchNode, signals } from "../store.js";

const props = defineProps({
  signal: { type: String, required: true },
  node: { type: Object, required: true },
});

const spec = computed(() => NODE_TYPES[props.node.type]);
const meter = computed(() => live.meters[props.signal]?.nodes?.[props.node.id] || null);
const set = (body) => patchNode(props.signal, props.node.id, body);
const active = ref(0);
// The band shown: the picked one, or the last when bands went away.
const shown = computed(() => Math.max(0, Math.min(active.value, (props.node.bands?.length || 1) - 1)));
const others = computed(() => signals.value.filter((s) => s.id !== props.signal));
const visible = computed(() => spec.value.fields.filter((f) => !f.when || f.when(props.node)));
const EQ = new Set(["eq", "dynamic-eq", "filter"]);
const DYN = new Set(["compressor", "limiter", "expander", "gate"]);

function setBand(i, key, value) {
  const bands = props.node.bands.map((b) => ({ ...b }));
  bands[i][key] = value;
  // High- and low-pass bands carry a slope (D4); the others must not.
  if (key === "type") {
    if (PASS_BANDS.has(value)) bands[i].slope_db_per_oct ??= 12;
    else delete bands[i].slope_db_per_oct;
  }
  set({ bands });
}
function addBand() {
  const last = props.node.bands[props.node.bands.length - 1];
  const freq = Math.min(16000, (last?.freq_hz || 500) * 2);
  const band = props.node.type === "eq" ? { type: "bell", freq_hz: freq, q: 1, gain_db: 0 } : { ...last, freq_hz: freq };
  set({ bands: [...props.node.bands, band] });
  active.value = props.node.bands.length;
}
function removeBand(i) {
  set({ bands: props.node.bands.filter((_, k) => k !== i) });
  active.value = Math.max(0, i - 1);
}
function setCrossover(i, v) {
  const crossovers_hz = [...props.node.crossovers_hz];
  crossovers_hz[i] = v;
  set({ crossovers_hz });
}
function setMbBand(i, key, v) {
  const bands = props.node.bands.map((b) => ({ ...b }));
  bands[i][key] = v;
  set({ bands });
}
function setBandCount(n) {
  const cur = props.node.crossovers_hz;
  const crossovers_hz = Array.from({ length: n - 1 }, (_, i) => cur[i] ?? Math.round(100 * Math.pow(10, (i + 1) * (2.3 / n))));
  crossovers_hz.sort((a, b) => a - b);
  const bands = Array.from({ length: n }, (_, i) => props.node.bands[i] ?? { ...props.node.bands[props.node.bands.length - 1] });
  set({ crossovers_hz, bands });
}
function setKey(field, part, value) {
  const key = { ...(props.node[field.key] || { signal: others.value[0]?.id || "", tap: "post-fader" }), [part]: value };
  set({ [field.key]: key });
}
function toggleOptional(field, on) {
  if (field.key_source) set({ [field.key]: on ? { signal: others.value[0]?.id || "", tap: "post-fader" } : null });
  else set({ [field.key]: on ? field.def : null });
}
</script>

<template>
  <div class="editor">
    <EqCurve v-if="EQ.has(node.type)" :node="node" :active="shown" @select="active = $event" @bands="set" />
    <SurroundPad v-if="node.type === 'surround-pan'" :node="node" @change="set" />
    <div v-if="DYN.has(node.type)" class="row tight" style="align-items: flex-start">
      <DynCurve :node="node" :meter="meter" />
      <div class="grbar" style="flex-direction: column; align-items: flex-start">
        <span class="muted small">Gain reduction</span>
        <span class="gr"><span :style="{ width: `${Math.min(100, ((meter?.gain_reduction_db || 0) / 20) * 100)}%` }"></span></span>
        <span class="num">{{ (meter?.gain_reduction_db || 0).toFixed(1) }} dB</span>
      </div>
    </div>
    <div v-else-if="meter && !['eq', 'filter', 'delay', 'width', 'surround-pan'].includes(node.type)" class="grbar">
      <span class="muted small">Gain reduction</span>
      <span class="gr"><span :style="{ width: `${Math.min(100, (meter.gain_reduction_db / 20) * 100)}%` }"></span></span>
      <span class="num">{{ meter.gain_reduction_db.toFixed(1) }} dB</span>
    </div>

    <div class="params">
      <template v-for="f in visible" :key="f.key">
        <div v-if="f.options">
          <label>{{ f.label }}</label>
          <Segmented :model-value="node[f.key]" :options="f.options.map(([value, label]) => ({ value, label }))" :label="f.label" @update:model-value="(v) => set({ [f.key]: v })" />
        </div>
        <label v-else-if="f.bool" class="check" :class="{ listen: f.key === 'listen' }">
          <input type="checkbox" :checked="node[f.key]" @change="set({ [f.key]: $event.target.checked })" />
          <span :class="{ 'state-tag bypass': f.key === 'listen' && node.listen }">{{ f.label }}</span>
        </label>
        <div v-else-if="f.key_source">
          <label class="check" style="margin-top: 0.5rem">
            <input v-if="f.optional" type="checkbox" :checked="!!node[f.key]" @change="toggleOptional(f, $event.target.checked)" />
            {{ f.label }}
          </label>
          <div v-if="node[f.key]" class="row tight">
            <select :value="node[f.key].signal" :aria-label="`${f.label} signal`" @change="setKey(f, 'signal', $event.target.value)">
              <option v-for="s in others" :key="s.id" :value="s.id">{{ s.name }}</option>
            </select>
            <select :value="typeof node[f.key].tap === 'string' ? node[f.key].tap : 'post-fader'" :aria-label="`${f.label} tap`" @change="setKey(f, 'tap', $event.target.value)">
              <option v-for="[v, l] in TAPS" :key="v" :value="v">{{ l }}</option>
            </select>
          </div>
        </div>
        <div v-else-if="f.optional">
          <label class="check" style="margin-top: 0.5rem">
            <input type="checkbox" :checked="node[f.key] !== null && node[f.key] !== undefined" @change="toggleOptional(f, $event.target.checked)" />
            {{ f.label }}
          </label>
          <Pot v-if="node[f.key] !== null && node[f.key] !== undefined" :label="f.label" :value="node[f.key]" v-bind="f" @change="(v) => set({ [f.key]: v })" />
        </div>
        <Pot v-else :label="f.label" :value="node[f.key] ?? f.def" :min="f.min" :max="f.max" :def="f.def" :fine="f.fine" :digits="f.digits ?? 1" :unit="f.unit || ''" :log="!!f.log" @change="(v) => set({ [f.key]: v })" />
      </template>
    </div>

    <div v-if="node.type === 'eq' || node.type === 'dynamic-eq'" class="bands">
      <div class="bandtabs" role="group" aria-label="Bands">
        <button v-for="(b, i) in node.bands" :key="i" class="btn small" :class="i === shown ? '' : 'secondary'" :aria-pressed="i === shown" :aria-label="`Band ${i + 1}`" @click="active = i">{{ i + 1 }}</button>
        <button v-if="node.bands.length < 8" class="btn small secondary" aria-label="Add a band" @click="addBand">+ Band</button>
      </div>
      <template v-for="(b, i) in node.bands" :key="i">
        <div v-if="i === shown" class="bandbox active">
          <h4>
            Band {{ i + 1 }}
            <select v-if="node.type === 'eq'" :value="b.type" style="width: auto; margin-left: auto" :aria-label="`band ${i + 1} type`" @change="setBand(i, 'type', $event.target.value)">
              <option v-for="[v, l] in BAND_TYPES" :key="v" :value="v">{{ l }}</option>
            </select>
            <button class="btn small secondary" :style="node.type === 'eq' ? '' : 'margin-left: auto'" :disabled="node.bands.length <= 1" :aria-label="`Remove band ${i + 1}`" @click="removeBand(i)">✕</button>
          </h4>
          <div class="params">
            <Pot label="Frequency" unit="Hz" :value="b.freq_hz" :min="10" :max="20000" :def="1000" :fine="0.02" :digits="0" log @change="(v) => setBand(i, 'freq_hz', v)" />
            <div v-if="PASS_BANDS.has(b.type)">
              <label>Slope dB/oct</label>
              <Segmented :model-value="b.slope_db_per_oct" :options="SLOPES.map((v) => ({ value: v, label: String(v) }))" :label="`band ${i + 1} slope`" @update:model-value="(v) => setBand(i, 'slope_db_per_oct', v)" />
            </div>
            <template v-else>
              <Pot v-if="b.type !== 'notch'" label="Gain" unit="dB" :value="b.gain_db" :min="-24" :max="24" :def="0" :fine="0.5" @change="(v) => setBand(i, 'gain_db', v)" />
              <Pot label="Q" :value="b.q" :min="0.1" :max="40" :def="1" :fine="0.05" :digits="2" log @change="(v) => setBand(i, 'q', v)" />
            </template>
            <template v-if="node.type === 'dynamic-eq'">
              <Pot label="Threshold" unit="dB" :value="b.threshold_db" :min="-100" :max="0" :def="-30" :fine="0.5" @change="(v) => setBand(i, 'threshold_db', v)" />
              <Pot label="Ratio" unit=": 1" :value="b.ratio" :min="1" :max="100" :def="3" :fine="0.05" log @change="(v) => setBand(i, 'ratio', v)" />
              <Pot label="Attack" unit="ms" :value="b.attack_ms" :min="0.1" :max="1000" :def="5" :fine="0.1" log @change="(v) => setBand(i, 'attack_ms', v)" />
              <Pot label="Release" unit="ms" :value="b.release_ms" :min="1" :max="10000" :def="100" :fine="0.1" :digits="0" log @change="(v) => setBand(i, 'release_ms', v)" />
            </template>
          </div>
        </div>
      </template>
    </div>

    <div v-if="node.type === 'multiband'">
      <label>Bands</label>
      <Segmented :model-value="node.bands.length" :options="[3, 4, 5].map((n) => ({ value: n, label: String(n) }))" label="Bands" @update:model-value="setBandCount" />
      <div class="params">
        <Pot v-for="(c, i) in node.crossovers_hz" :key="`x${i}`" :label="`Crossover ${i + 1}`" unit="Hz" :value="c" :min="20" :max="20000" :def="c" :fine="0.02" :digits="0" log @change="(v) => setCrossover(i, v)" />
      </div>
      <div class="bands">
        <div class="bandtabs" role="group" aria-label="Bands">
          <button v-for="(b, i) in node.bands" :key="i" class="btn small" :class="i === shown ? '' : 'secondary'" :aria-pressed="i === shown" :aria-label="`Band ${i + 1}`" @click="active = i">{{ i + 1 }}</button>
        </div>
        <template v-for="(b, i) in node.bands" :key="i">
          <div v-if="i === shown" class="bandbox active">
            <h4>Band {{ i + 1 }}</h4>
            <div class="params">
              <Pot label="Threshold" unit="dB" :value="b.threshold_db" :min="-100" :max="0" :def="-20" :fine="0.5" @change="(v) => setMbBand(i, 'threshold_db', v)" />
              <Pot label="Ratio" unit=": 1" :value="b.ratio" :min="1" :max="100" :def="2" :fine="0.05" log @change="(v) => setMbBand(i, 'ratio', v)" />
              <Pot label="Attack" unit="ms" :value="b.attack_ms" :min="0.1" :max="1000" :def="10" :fine="0.1" log @change="(v) => setMbBand(i, 'attack_ms', v)" />
              <Pot label="Release" unit="ms" :value="b.release_ms" :min="1" :max="10000" :def="150" :fine="0.1" :digits="0" log @change="(v) => setMbBand(i, 'release_ms', v)" />
              <Pot label="Makeup" unit="dB" :value="b.makeup_db" :min="-24" :max="24" :def="0" :fine="0.5" @change="(v) => setMbBand(i, 'makeup_db', v)" />
            </div>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>
