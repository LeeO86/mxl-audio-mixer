<script setup>
// An unfolded strip (A13): one fader, trim, mute and meter per channel (member). The bar above them shows
// whether they are linked (the default): linked, moving one moves the strip, so all follow and keep their
// offsets; unlinked, each moves on its own and keeps an offset from the strip's fader and trim (stored in
// the strip's `members`). The pan is never linked (the surround panner has a position per channel), the
// processing is always shared.
import { computed } from "vue";
import Meter from "./Meter.vue";
import MiniPot from "./MiniPot.vue";
import VFader from "./VFader.vue";
import { fmtSigned } from "../api.js";
import { channelLabels, hasOffsets, memberChange, memberValue } from "../console.js";
import { patchSignal } from "../store.js";

const props = defineProps({
  signal: { type: Object, required: true },
  meters: { type: Object, default: null },
  scale: { type: String, default: "dbfs" },
  alignment: { type: Number, default: -18 },
  colour: { type: String, default: "" },
  muted: { type: Boolean, default: false },
});
const s = computed(() => props.signal);
const labels = computed(() => channelLabels(s.value.format));
const n = computed(() => s.value.patch.length);
const members = computed(() => Array.from({ length: n.value }, (_, ch) => ({ ch, label: labels.value[ch] || String(ch + 1), ...memberValue(s.value, n.value, ch) })));
const linked = computed(() => !s.value.unlinked);
const offsets = computed(() => hasOffsets(s.value));

function change(ch, field, value) {
  const body = memberChange(s.value, n.value, ch, field, value);
  if (body) patchSignal(s.value.id, body);
}
const toggleLink = () => patchSignal(s.value.id, { unlinked: linked.value });
const resetOffsets = () => patchSignal(s.value.id, { members: [] });
// One channel of the strip's meters.
const meterOf = (ch) => props.meters && Object.fromEntries(Object.entries(props.meters).map(([k, v]) => [k, Array.isArray(v) ? [v[ch]] : v]));
</script>

<template>
  <div class="members" :data-members="s.id">
    <div class="linkrow">
      <button
        class="linkbar"
        :class="{ on: linked }"
        :aria-pressed="linked"
        :aria-label="`${s.name} members linked`"
        :title="linked ? 'linked: a member moves them all (offsets kept); click to unlink' : 'unlinked: every member moves on its own; click to link'"
        @click="toggleLink"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path v-if="linked" d="M10 14a4 4 0 0 1 0-5.7l3-3a4 4 0 0 1 5.7 5.7l-1.5 1.5M14 10a4 4 0 0 1 0 5.7l-3 3a4 4 0 0 1-5.7-5.7l1.5-1.5" />
          <path v-else d="M9 15l-2.5 2.5a3 3 0 0 1-4.2-4.2L5 10.6M15 9l2.5-2.5a3 3 0 0 1 4.2 4.2L19 13.4M8 4l1 3M4 8l3 1M16 20l-1-3M20 16l-3-1" />
        </svg>
        {{ linked ? "Linked" : "Unlinked" }}
      </button>
      <button v-if="offsets" class="btn small secondary" :aria-label="`${s.name} clear member offsets`" title="every member back to the strip's fader and trim, no member mute" @click="resetOffsets">0</button>
    </div>
    <div class="memcols">
      <div v-for="m in members" :key="m.ch" class="memcol" :data-member="m.label">
        <div class="memhead">
          <span class="memlabel">{{ m.label }}</span>
          <button class="btn small memmute" :class="m.mute ? 'danger' : 'secondary'" :aria-pressed="m.mute" :aria-label="`${s.name} ${m.label} mute`" @click="change(m.ch, 'mute', !m.mute)">M</button>
        </div>
        <MiniPot label="Tr" :title="`${m.label} trim ${fmtSigned(m.trim_db)} dB: double-click 0`" :value="m.trim_db" :min="-42" :max="42" :step="0.5" :format="(v) => fmtSigned(v, 0)" @change="(v) => change(m.ch, 'trim_db', v)" />
        <div class="memfader">
          <VFader :value="m.fader_db" :label="`${s.name} ${m.label} fader`" :colour="colour" @change="(v) => change(m.ch, 'fader_db', v)" />
          <Meter :signal="s.id" :values="meterOf(m.ch)" :channels="1" :scale="scale" :alignment="alignment" :marks="false" :muted="muted || m.mute" />
        </div>
      </div>
    </div>
  </div>
</template>
