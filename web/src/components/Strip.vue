<script setup>
// One console strip (§12.1): name and colour, input patch, trim, polarity, the chain overview (response
// curve, nodes with gain reduction), sends, pan and width, mute, the fader and the meter. Buses have no
// patch, trim or pan. Every change goes to the API; the strip follows the mix pushed back. `compact` (the
// strip and fader-bank widgets) keeps the name, gain reduction, mute, fader and meter. The name shows the
// tally of the strip's index: red on air, green on preview.
import { computed } from "vue";
import Meter from "./Meter.vue";
import MiniCurve from "./MiniCurve.vue";
import MiniPot from "./MiniPot.vue";
import VFader from "./VFader.vue";
import { dbValue, fmtPan, fmtSigned } from "../api.js";
import { DYNAMICS } from "../nodes.js";
import { groupMuted, live, patchSend, patchSignal, select, sends, signalOf, soloed, tallyOf, toggleSolo, vcaMuted, vcasAbove } from "../store.js";

const props = defineProps({
  signal: { type: Object, required: true },
  bus: { type: Boolean, default: false },
  scale: { type: String, default: "dbfs" },
  alignment: { type: Number, default: -18 },
  compact: { type: Boolean, default: false },
});
const emit = defineEmits(["open"]);

const s = computed(() => props.signal);
const channels = computed(() => ({ mono: 1, stereo: 2, "3.0": 3, "5.1": 6, "7.1": 8, "5.1.4": 10, "7.1.4": 12 })[s.value.format] || (Array.isArray(s.value.format?.custom) ? s.value.format.custom.length : 2));
const meters = computed(() => live.meters[s.value.id] || null);
const colour = computed(() => s.value.colour || "");
const selected = computed(() => live.selected === s.value.id);
const mySends = computed(() => sends.value.filter((x) => x.from === s.value.id));
const latency = computed(() => live.latency[s.value.id] || 0);
const tally = computed(() => tallyOf(s.value.id));
const myVcas = computed(() => vcasAbove(s.value.id));
const otherMute = computed(() => (vcaMuted(s.value.id) ? "VCA" : groupMuted(s.value) ? "group" : ""));
const dynamics = computed(() => s.value.chain.filter((n) => DYNAMICS.has(n.type)));
function open() {
  if (props.compact) return;
  select(s.value.id);
  emit("open", s.value.id);
}

const inputs = computed(() => live.status?.inputs || []);
function patchValue(p) {
  if (!p) return "";
  return "talkback" in p ? `t:${p.talkback}` : `${p.input}:${p.channel}`;
}
function setPatch(ch, value) {
  const patch = [...s.value.patch];
  if (!value) patch[ch] = null;
  else if (value.startsWith("t:")) patch[ch] = { talkback: value.slice(2) };
  else {
    const [input, channel] = value.split(":").map(Number);
    patch[ch] = { input, channel };
  }
  patchSignal(s.value.id, { patch });
}
// Talkback mics (§10.2): the show's and the browsers' that are on, and any a strip already uses.
const talkOptions = computed(() => {
  const ids = new Set([
    ...(live.mix?.control_room?.talkback?.mics || []).map((m) => m.id),
    ...(live.status?.talkback?.browser_mics || []),
    ...s.value.patch.filter((p) => p && "talkback" in p).map((p) => p.talkback),
  ]);
  const name = (id) => live.mix?.control_room?.talkback?.mics?.find((m) => m.id === id)?.name || id;
  return [...ids].map((id) => ({ value: `t:${id}`, text: `TB ${name(id)}` }));
});
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
  <section
    class="strip"
    :class="{ bus, compact, selected: selected && !compact, muted: s.mute || !!otherMute, onair: tally?.on_air, preview: tally?.preview && !tally?.on_air }"
    :style="{ '--colour': colour || 'var(--border)' }"
    :data-signal="s.id"
  >
    <button class="sname" :title="compact ? `${s.name} (${s.id})` : `${s.name} (${s.id}); open its processing`" @click="open">
      <span class="nm">{{ s.name }}</span>
      <span class="fmt"
        >{{ s.format }}<span v-if="bus && s.minus" class="minus" :title="`mix-minus: ${busName(s.minus.of)} without ${s.minus.without.map((w) => signalOf(w)?.name || w).join(', ')}`"> −{{ busName(s.minus.of) }}</span><span v-if="s.recall_safe" class="safe" title="recall-safe"> S</span
        ><span v-if="tally?.afv" class="afv" :class="{ open: tally.afv_open }" :title="`audio follows video: tally ${tally.index}, ${tally.afv_open ? 'open' : 'closed'}`"> AFV</span></span
      >
    </button>

    <div v-if="compact" class="grlist">
      <span v-for="n in dynamics" :key="n.id" class="node" :title="`${n.id}: gain reduction ${nodeGr(n).toFixed(1)} dB`">
        <span class="nid">{{ n.id }}</span>
        <span class="gr"><span :style="{ width: `${Math.min(100, (nodeGr(n) / 20) * 100)}%` }"></span></span>
      </span>
    </div>
    <div v-else class="upper">
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
        <option v-for="o in talkOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
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

    <button class="chainview" :title="`${s.chain.length} processing nodes; open the editor`" @click="open">
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

    <div class="solo">
      <button class="btn small" :class="soloed[s.id] === 'pfl' ? 'pfl' : 'secondary'" :aria-pressed="soloed[s.id] === 'pfl'" :aria-label="`PFL ${s.name}`" title="pre-fader listen" @click="toggleSolo(s.id, 'pfl')">PFL</button>
      <button v-if="!compact" class="btn small" :class="soloed[s.id] === 'afl' ? 'pfl' : 'secondary'" :aria-pressed="soloed[s.id] === 'afl'" :aria-label="`AFL ${s.name}`" title="after-fader listen" @click="toggleSolo(s.id, 'afl')">AFL</button>
    </div>
    <button class="btn mute" :class="[s.mute ? 'danger' : 'secondary', { othermute: !s.mute && otherMute }]" :aria-pressed="s.mute" :title="otherMute ? `muted by a ${otherMute === 'VCA' ? 'VCA' : 'mute group'}` : ''" @click="patchSignal(s.id, { mute: !s.mute })">Mute</button>
    <div v-if="myVcas.length && !compact" class="vcachips muted small" :title="`VCAs: ${myVcas.map((v) => v.name).join(', ')}`">
      <span v-for="v in myVcas" :key="v.id" class="vcachip" :style="{ borderColor: v.colour || 'var(--border)' }">{{ v.name }}</span>
    </div>

    <div class="faderrow">
      <VFader :value="s.fader_db" :label="`${s.name} fader`" :colour="colour" @change="(v) => patchSignal(s.id, { fader_db: v })" />
      <Meter :signal="s.id" :values="meters" :channels="channels" :scale="scale" :alignment="alignment" :marks="false" />
    </div>
    <div v-if="!compact" class="sfoot muted small" :title="`processing latency of ${s.name} at its post-fader point`">{{ latency ? `${latency.toFixed(2)} ms` : " " }}</div>
  </section>
</template>
