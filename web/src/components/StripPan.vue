<script setup>
// A strip's pan (A5–A8). The strip shows the L/R pan only, whatever its format: a mono strip's pan, a
// stereo strip's balance (and width), a wider strip's balance of its left and right channels. A strip
// whose post-pan sends feed a 5.1, 7.1, 5.1.4 or 7.1.4 bus has a surround button: it opens the surround
// panner (`surround` on the strip; it pans those sends, the L/R pan the others). A mono or stereo strip
// has one position; a wider strip one per channel (the LFE stays 1:1), and "Restore 1:1" removes them.
import { computed, ref } from "vue";
import MiniPot from "./MiniPot.vue";
import Modal from "./Modal.vue";
import SurroundPad from "./SurroundPad.vue";
import { fmtPan } from "../api.js";
import { channelLabels } from "../console.js";
import { HEIGHT_LAYOUTS } from "../nodes.js";
import { sendPanBed } from "../sendpan.js";
import { isSubgroup, patchSignal, sends, signalOf } from "../store.js";

const props = defineProps({ signal: { type: Object, required: true } });
const s = computed(() => props.signal);
const open = ref(false);

// The widest surround bus a post-pan send of the strip feeds (the panner pans into each in its layout),
// as the server's Mix::strip_surround: not a send whose own pan applies (B6), not one with a block delay.
const LAYOUTS = ["7.1.4", "5.1.4", "7.1", "5.1"];
const postPan = computed(() => sends.value.filter((x) => x.from === s.value.id && (x.tap === "post-pan" || isSubgroup(x.to)) && LAYOUTS.includes(signalOf(x.to)?.format)));
const layouts = computed(() => {
  const set = new Set(postPan.value.filter((x) => !x.block_delay && !(x.pan && sendPanBed(x))).map((x) => signalOf(x.to)?.format));
  return LAYOUTS.filter((f) => set.has(f));
});
// Post-pan sends into surround buses the panner does not pan: their own pan, or a block delay (A7 review).
const unpanned = computed(() =>
  postPan.value
    .filter((x) => x.block_delay || (x.pan && sendPanBed(x)))
    .map((x) => `${signalOf(x.to)?.name || x.to} (${x.block_delay ? "block delay: 1:1" : "its own send pan"})`),
);
const layout = computed(() => layouts.value[0] || LAYOUTS.find((f) => postPan.value.some((x) => signalOf(x.to)?.format === f)) || "5.1");
const narrow = computed(() => s.value.format === "mono" || s.value.format === "stereo");
const positions = computed(() => s.value.surround || []);
const active = computed(() => positions.value.some(Boolean));
const offered = computed(() => postPan.value.length > 0 || active.value);
const height = computed(() => HEIGHT_LAYOUTS.has(layout.value));

const fmtAz = (a) => {
  const v = Math.round(a || 0);
  return v === 0 ? "front" : Math.abs(v) === 180 ? "rear" : `${v < 0 ? "L" : "R"} ${Math.abs(v)}°`;
};
const buttonText = computed(() => {
  if (!active.value) return "1:1";
  if (narrow.value) return fmtAz(positions.value[0].azimuth_deg);
  return `${positions.value.filter(Boolean).length} of ${s.value.surround.length - lfeCount.value}`;
});

// A new position: front centre, no spread, no divergence, LFE off.
const DEFAULT = { azimuth_deg: 0, elevation_deg: 0, spread_pct: 0, divergence_pct: 0, lfe_db: -100 };
// Where a wider strip's channel sits 1:1 (its speaker), as SurroundPad draws it; the tops are up.
const NATURAL = { L: -30, R: 30, C: 0, Ls: -110, Rs: 110, Lss: -90, Rss: 90, Lrs: -150, Rrs: 150, Ltf: -45, Rtf: 45, Ltr: -135, Rtr: 135 };
const TOPS = new Set(["Ltf", "Rtf", "Ltr", "Rtr"]);
const labels = computed(() => channelLabels(s.value.format));
const lfeCount = computed(() => labels.value.filter((l) => l === "LFE").length);
const natural = (ch) => {
  const l = labels.value[ch];
  return { ...DEFAULT, azimuth_deg: NATURAL[l] ?? 0, elevation_deg: TOPS.has(l) ? 90 : 0 };
};
const channels = computed(() => labels.value.map((label, ch) => ({ ch, label, pos: positions.value[ch] || null })).filter((c) => c.label !== "LFE"));

function entries() {
  const n = narrow.value ? 1 : labels.value.length;
  const cur = positions.value.length === n ? positions.value : Array(n).fill(null);
  return cur.map((p) => (p ? { ...p } : null));
}
/** Change position `ch` (0 for a mono or stereo strip) by `body`. */
function setPos(ch, body) {
  const all = entries();
  all[ch] = { ...(all[ch] || (narrow.value ? DEFAULT : natural(ch))), ...body };
  patchSignal(s.value.id, { surround: all });
}
function oneToOne(ch) {
  const all = entries();
  all[ch] = null;
  patchSignal(s.value.id, { surround: all.some(Boolean) ? all : [] });
}
const restore = () => patchSignal(s.value.id, { surround: [] });
const padNode = (p) => ({ layout: layout.value, ...DEFAULT, ...p });
</script>

<template>
  <div class="strippan">
    <MiniPot v-if="s.format === 'stereo'" label="Width" :value="s.width" :min="0" :max="2" :step="0.01" :def="1" :format="(v) => `${Math.round(v * 100)}%`" @change="(v) => patchSignal(s.id, { width: v })" />
    <button
      v-if="offered"
      class="sendval surbtn"
      :class="{ on: active }"
      :data-surround="active ? 'on' : 'off'"
      :aria-label="`${s.name} surround pan`"
      :title="`surround panner into ${layouts.join(', ') || layout} (the post-pan sends to surround buses): click to open`"
      @click="open = true"
    >
      <span>◎ {{ layout }}</span><span class="num">{{ buttonText }}</span>
    </button>
    <MiniPot
      data-learn="pan"
      :label="s.format === 'mono' ? 'Pan' : 'Bal'"
      :title="s.format === 'mono' ? 'pan L/R: double-click resets' : 'balance L/R: double-click resets'"
      :value="s.pan"
      :min="-1"
      :max="1"
      :step="0.01"
      :def="0"
      :format="fmtPan"
      @change="(v) => patchSignal(s.id, { pan: v })"
    />

    <Modal v-if="open" :title="`Surround pan: ${s.name} into ${layout}`" :wide="!narrow" @close="open = false">
      <p class="note">
        Pans the post-pan sends into {{ layouts.length ? layouts.join(", ") : "surround buses" }}, each in its own layout (heights on 5.1.4 and 7.1.4 only); a send with a pan of
        its own keeps it. The L/R pan on the strip feeds every other bus.
      </p>
      <p v-if="unpanned.length" class="warnbox" data-unpanned>Not panned here: {{ unpanned.join(", ") }}.</p>
      <template v-if="narrow">
        <div class="surround1">
          <SurroundPad :node="padNode(positions[0])" :size="220" @change="(b) => setPos(0, b)" />
          <div class="surpots">
            <MiniPot v-if="height" label="Height" :value="positions[0]?.elevation_deg || 0" :min="0" :max="90" :step="1" :def="0" :format="(v) => `${Math.round(v)}°`" @change="(v) => setPos(0, { elevation_deg: v })" />
            <MiniPot label="Spread" :value="positions[0]?.spread_pct || 0" :min="0" :max="100" :step="1" :def="0" :format="(v) => `${Math.round(v)}%`" @change="(v) => setPos(0, { spread_pct: v })" />
            <MiniPot label="Divergence" :value="positions[0]?.divergence_pct || 0" :min="0" :max="100" :step="1" :def="0" :format="(v) => `${Math.round(v)}%`" @change="(v) => setPos(0, { divergence_pct: v })" />
            <MiniPot label="LFE" :value="positions[0]?.lfe_db ?? -100" :min="-100" :max="10" :step="0.5" :def="-100" :format="(v) => (v <= -100 ? 'off' : `${v.toFixed(1)} dB`)" @change="(v) => setPos(0, { lfe_db: v })" />
          </div>
        </div>
      </template>
      <div v-else class="surroundgrid">
        <div v-for="c in channels" :key="c.ch" class="surcell" :class="{ moved: c.pos }" :data-channel="c.label">
          <div class="surhead">
            <strong>{{ c.label }}</strong>
            <button v-if="c.pos" class="btn small secondary" :aria-label="`${s.name} ${c.label} 1:1`" title="back to its own speaker (1:1)" @click="oneToOne(c.ch)">1:1</button>
            <span v-else class="muted small">1:1</span>
          </div>
          <SurroundPad :node="padNode(c.pos || natural(c.ch))" :size="120" mini @change="(b) => setPos(c.ch, b)" />
          <MiniPot v-if="height" label="Height" :value="(c.pos || natural(c.ch)).elevation_deg" :min="0" :max="90" :step="1" :def="0" :format="(v) => `${Math.round(v)}°`" @change="(v) => setPos(c.ch, { elevation_deg: v })" />
        </div>
      </div>
      <div class="actions">
        <button class="btn secondary" :disabled="!active" title="every channel to its own speaker again; the strip's post-pan sends map 1:1 (up- or down-mixed)" @click="restore">Restore 1:1</button>
        <button class="btn secondary" @click="open = false">Close</button>
      </div>
    </Modal>
  </div>
</template>
