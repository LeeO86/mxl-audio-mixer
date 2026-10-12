<script setup>
// One console strip (§12.1): name and colour, input patch, trim, polarity, the chain overview (response
// curve, nodes with gain reduction), sends, pan and width, mute, the fader and the meter. Buses have no
// patch, trim or pan. Every change goes to the API; the strip follows the mix pushed back.
import { computed } from "vue";
import Meter from "./Meter.vue";
import MiniCurve from "./MiniCurve.vue";
import MiniPot from "./MiniPot.vue";
import VFader from "./VFader.vue";
import { dbValue, fmtPan, fmtSigned } from "../api.js";
import { DYNAMICS } from "../nodes.js";
import { live, patchSend, patchSignal, select, sends, signalOf } from "../store.js";

const props = defineProps({
  signal: { type: Object, required: true },
  bus: { type: Boolean, default: false },
  scale: { type: String, default: "dbfs" },
  alignment: { type: Number, default: -18 },
});
const emit = defineEmits(["open"]);

const s = computed(() => props.signal);
const channels = computed(() => ({ mono: 1, stereo: 2, "3.0": 3, "5.1": 6, "7.1": 8, "5.1.4": 10, "7.1.4": 12 })[s.value.format] || (Array.isArray(s.value.format?.custom) ? s.value.format.custom.length : 2));
const meters = computed(() => live.meters[s.value.id] || null);
const colour = computed(() => s.value.colour || "");
const selected = computed(() => live.selected === s.value.id);
const mySends = computed(() => sends.value.filter((x) => x.from === s.value.id));
const latency = computed(() => live.latency[s.value.id] || 0);

const inputs = computed(() => live.status?.inputs || []);
function patchValue(p) {
  return p ? `${p.input}:${p.channel}` : "";
}
function setPatch(ch, value) {
  const patch = [...s.value.patch];
  if (!value) patch[ch] = null;
  else {
    const [input, channel] = value.split(":").map(Number);
    patch[ch] = { input, channel };
  }
  patchSignal(s.value.id, { patch });
}
const patchOptions = computed(() =>
  inputs.value.flatMap((i) =>
    Array.from({ length: Math.max(i.channels || 0, 2) }, (_, c) => ({ value: `${i.input}:${c + 1}`, text: `${i.input}·${c + 1}`, dim: !i.channels || c >= i.channels })),
  ),
);
const patchState = (ch) => s.value.patch_state?.[ch] || (s.value.patch?.[ch] ? "ok" : "unpatched");

function togglePolarity(ch) {
  const polarity = [...s.value.polarity];
  polarity[ch] = !polarity[ch];
  patchSignal(s.value.id, { polarity });
}

const nodeGr = (n) => meters.value?.nodes?.[n.id]?.gain_reduction_db || 0;
const busName = (id) => signalOf(id)?.name || id;
const sendDb = (x) => {
  const v = dbValue(x.gain_db);
  return Number.isFinite(v) ? Math.max(-60, v) : -60;
};
const setSend = (x, v) => patchSend(x.from, x.to, { gain_db: v <= -60 ? "-inf" : v });
const fmtSend = (v) => (v <= -60 ? "−∞" : fmtSigned(v));
</script>

<template>
  <section class="strip" :class="{ bus, selected, muted: s.mute }" :style="{ '--colour': colour || 'var(--border)' }" :data-signal="s.id">
    <button class="sname" :title="`${s.name} (${s.id}); open its processing`" @click="select(s.id), emit('open', s.id)">
      <span class="nm">{{ s.name }}</span>
      <span class="fmt">{{ typeof s.format === "string" ? s.format : "custom" }}<span v-if="s.recall_safe" class="safe" title="recall-safe"> S</span></span>
    </button>

    <div class="upper">
    <div v-if="!bus" class="patch">
      <select
        v-for="(p, ch) in s.patch"
        :key="ch"
        :class="patchState(ch)"
        :value="patchValue(p)"
        :aria-label="`${s.name} channel ${ch + 1} input`"
        :title="`channel ${ch + 1}: ${patchState(ch)}`"
        @change="setPatch(ch, $event.target.value)"
      >
        <option value="">—</option>
        <option v-for="o in patchOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
      </select>
    </div>

    <template v-if="!bus">
      <MiniPot label="Trim" :value="s.trim_db" :min="-42" :max="42" :step="0.5" :format="(v) => fmtSigned(v)" @change="(v) => patchSignal(s.id, { trim_db: v })" />
      <div class="polarity">
        <button
          v-for="(p, ch) in s.polarity"
          :key="ch"
          class="btn small"
          :class="p ? 'bypass' : 'secondary'"
          :aria-pressed="p"
          :aria-label="`${s.name} channel ${ch + 1} polarity`"
          @click="togglePolarity(ch)"
        >
          Ø{{ s.polarity.length > 1 ? ch + 1 : "" }}
        </button>
      </div>
    </template>

    <button class="chainview" :title="`${s.chain.length} processing nodes; open the editor`" @click="select(s.id), emit('open', s.id)">
      <MiniCurve :chain="s.chain" />
      <span v-if="!s.chain.length" class="muted small">no processing</span>
      <span v-for="n in s.chain" :key="n.id" class="node" :class="{ bypassed: n.bypass }">
        <span class="nid">{{ n.id }}</span>
        <span v-if="DYNAMICS.has(n.type)" class="gr" :title="`gain reduction ${nodeGr(n).toFixed(1)} dB`"><span :style="{ width: `${Math.min(100, (nodeGr(n) / 20) * 100)}%` }"></span></span>
      </span>
    </button>

    <div class="sends">
      <MiniPot v-for="x in mySends" :key="x.to" :label="busName(x.to)" :value="sendDb(x)" :min="-60" :max="10" :step="0.5" :def="0" :format="fmtSend" :title="`send to ${busName(x.to)}`" :class="{ off: x.mute }" @change="(v) => setSend(x, v)" />
    </div>

    <template v-if="!bus && (s.format === 'mono' || s.format === 'stereo')">
      <MiniPot :label="s.format === 'stereo' ? 'Bal' : 'Pan'" :value="s.pan" :min="-1" :max="1" :step="0.01" :def="0" :format="fmtPan" @change="(v) => patchSignal(s.id, { pan: v })" />
      <MiniPot v-if="s.format === 'stereo'" label="Width" :value="s.width" :min="0" :max="2" :step="0.01" :def="1" :format="(v) => `${Math.round(v * 100)}%`" @change="(v) => patchSignal(s.id, { width: v })" />
    </template>
    </div>

    <button class="btn mute" :class="s.mute ? 'danger' : 'secondary'" :aria-pressed="s.mute" @click="patchSignal(s.id, { mute: !s.mute })">Mute</button>

    <div class="faderrow">
      <VFader :value="s.fader_db" :label="`${s.name} fader`" :colour="colour" @change="(v) => patchSignal(s.id, { fader_db: v })" />
      <Meter :signal="s.id" :values="meters" :channels="channels" :scale="scale" :alignment="alignment" :marks="false" />
    </div>
    <div class="sfoot muted small" :title="`processing latency of ${s.name} at its post-fader point`">{{ latency ? `${latency.toFixed(2)} ms` : " " }}</div>
  </section>
</template>
