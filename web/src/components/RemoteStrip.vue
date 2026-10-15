<script setup>
// One strip of a far-side desk (§13). It looks like a local strip. Fader, mute and
// auxiliary sends go to that mixer; the meter is the peak it reports.
import { computed } from "vue";
import Meter from "./Meter.vue";
import MiniPot from "./MiniPot.vue";
import VFader from "./VFader.vue";
import { dbValue, fmtSigned } from "../api.js";

const props = defineProps({
  signal: { type: Object, required: true },
  bus: { type: Boolean, default: false },
  sends: { type: Array, default: () => [] },
  names: { type: Object, default: () => ({}) },
  peak: { type: Number, default: null },
  scale: { type: String, default: "dbfs" },
  alignment: { type: Number, default: -18 },
});
const emit = defineEmits(["fader", "mute", "send"]);

const s = computed(() => props.signal);
const channels = computed(() => ({ mono: 1, stereo: 2, "3.0": 3, "5.1": 6, "7.1": 8, "5.1.4": 10, "7.1.4": 12 })[s.value.format] || 1);
const meters = computed(() => (Number.isFinite(props.peak) ? { peak_dbfs: [props.peak], rms_dbfs: [props.peak], peak_hold_dbfs: [props.peak] } : null));
const patch = computed(() => (s.value.patch || []).filter(Boolean).join(" "));
const sendDb = (x) => {
  const v = dbValue(x.gain_db);
  return Number.isFinite(v) ? Math.max(-60, v) : -60;
};
const fmtSend = (v) => (v <= -60 ? "−∞" : fmtSigned(v));
</script>

<template>
  <section class="strip remote" :class="{ bus, muted: s.mute, ears: bus && s.ears }" :style="{ '--colour': s.colour || 'var(--border)' }" :data-signal="s.id">
    <div class="sname" :title="`${s.name} (${s.id}) on the far side`">
      <span class="nm">{{ s.name }}</span>
      <span class="fmt">{{ s.format }}<span v-if="bus && s.ears"> · in-ear</span><span v-if="bus && s.minus"> · minus</span></span>
    </div>
    <div class="upper">
      <div v-if="patch" class="patchlabel muted small" :title="`patched from ${patch}`">in {{ patch }}</div>
      <div class="sends">
        <div v-for="x in sends" :key="x.to" class="rsend">
          <MiniPot :label="names[x.to] || x.to" :value="sendDb(x)" :min="-60" :max="10" :step="0.5" :def="0" :format="fmtSend" :title="`send to ${names[x.to] || x.to}${x.pre ? ', pre-fader' : ''}`" :class="{ off: x.mute }" @change="(v) => emit('send', x, v <= -60 ? '-inf' : v)" />
          <button class="btn small" :class="x.mute ? 'danger' : 'secondary'" :aria-pressed="x.mute" :aria-label="`Mute send to ${names[x.to] || x.to}`" @click="emit('send', x, x.gain_db, !x.mute)">M</button>
        </div>
      </div>
    </div>
    <button class="btn mute" :class="s.mute ? 'danger' : 'secondary'" :aria-pressed="s.mute" @click="emit('mute', !s.mute)">Mute</button>
    <div class="faderrow">
      <VFader :value="s.fader_db" :label="`${s.name} fader`" :colour="s.colour || ''" @change="(v) => emit('fader', v)" />
      <Meter :signal="s.id" :values="meters" :channels="channels" :scale="scale" :alignment="alignment" :marks="false" />
    </div>
  </section>
</template>
