<script setup>
// One console strip (§12.1): name and colour, input patch, trim, polarity, the chain overview (response
// curve, nodes with gain reduction), sends, pan and width, mute, the fader and the meter. Buses have no
// patch, trim or pan. Every change goes to the API; the strip follows the mix pushed back. `compact` (the
// strip and fader-bank widgets) keeps the name, gain reduction, mute, fader and meter. The name shows the
// tally of the strip's index: red on air, green on preview. A click on the name renames the strip (C1);
// the gear opens its processing, the ⋮ menu routes it (C11), makes a bus a subgroup (C12) or deletes it
// (C9).
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import Meter from "./Meter.vue";
import MiniCurve from "./MiniCurve.vue";
import MiniPot from "./MiniPot.vue";
import Modal from "./Modal.vue";
import VFader from "./VFader.vue";
import { api, dbValue, fmtPan, fmtSigned } from "../api.js";
import { channelLabels, isLinked, linkedPatch } from "../console.js";
import { DYNAMICS } from "../nodes.js";
import {
  act,
  addSend,
  autoGain,
  automixOf,
  groupMuted,
  isBus,
  isSubgroup,
  live,
  patchSend,
  patchSignal,
  select,
  sendSources,
  sendTargets,
  sends,
  signalOf,
  signalPath,
  soloed,
  store,
  stored,
  tallyOf,
  toggleSolo,
  vcaMuted,
  vcasAbove,
} from "../store.js";

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
// An active surround panner (agent B's `surround-pan` node) replaces the strip's own pan.
const surroundPan = computed(() => s.value.chain.find((n) => n.type === "surround-pan" && !n.bypass) || null);
const dynamics = computed(() => s.value.chain.filter((n) => DYNAMICS.has(n.type)));
const vad = computed(() => live.vad[s.value.id] || null);
const auto = computed(() => autoGain(s.value.id));
const autoTitle = computed(() => {
  const am = automixOf(s.value.id);
  const g = Number.isFinite(auto.value) ? `${auto.value.toFixed(1)} dB` : "−∞";
  return `auto gain ${g} (${am ? `auto-mix ${am.name}` : "ducking"}), after the fader`;
});
function open() {
  if (props.compact) return;
  select(s.value.id);
  emit("open", s.value.id);
}

// Rename in place (C1): Enter or leaving the field saves, Escape cancels.
const editing = ref(false);
const draft = ref("");
const nameInput = ref(null);
function startRename() {
  if (props.compact) return;
  select(s.value.id);
  draft.value = s.value.name;
  editing.value = true;
  nextTick(() => nameInput.value?.select());
}
function endRename(save) {
  if (!editing.value) return;
  editing.value = false;
  const name = draft.value.trim();
  if (save && name && name !== s.value.name) patchSignal(s.value.id, { name });
}

// The strip menu.
const menu = ref(false);
const menuEl = ref(null);
const closeMenu = (e) => {
  if (!menuEl.value?.contains(e.target)) menu.value = false;
};
watch(menu, (on) => (on ? window.addEventListener("mousedown", closeMenu) : window.removeEventListener("mousedown", closeMenu)));
onUnmounted(() => window.removeEventListener("mousedown", closeMenu));
const dialog = ref(""); // "send-to", "add-source", "subgroup", "delete", "polarity"
function choose(what) {
  menu.value = false;
  if (what === "processing") open();
  else dialog.value = what;
}

// Routing from the console (C11): a new send at 0 dB from the source's output.
const targets = computed(() => (dialog.value === "send-to" ? sendTargets(s.value.id) : []));
const sources = computed(() => (dialog.value === "add-source" ? sendSources(s.value.id) : []));
async function route(from, to) {
  if (await addSend(from, to)) dialog.value = "";
}

// Subgroup (C12): every send into the bus post-fader at 0 dB.
const intoMe = computed(() => sends.value.filter((x) => x.to === s.value.id));
// The sends that change when it becomes one: not at 0 dB, or not from the source's output.
const relevelled = computed(() => intoMe.value.filter((x) => dbValue(x.gain_db) !== 0 || x.tap !== (isBus(x.from) ? "post-fader" : "post-pan")).length);
async function setSubgroup(on) {
  if (await act(() => api.patch(signalPath(s.value.id), { subgroup: on }), on ? `${s.value.name} is a subgroup.` : `${s.value.name} is a plain bus again.`)) dialog.value = "";
}

// Delete (C9): what goes with it.
const kind = computed(() => (props.bus ? "bus" : "strip"));
const refs = computed(() => {
  const id = s.value.id;
  const m = live.mix || {};
  const out = [];
  const n = sends.value.filter((x) => x.from === id || x.to === id).length;
  if (n) out.push(`${n} send${n === 1 ? "" : "s"}`);
  if ((m.outputs || []).some((o) => o.signal === id)) out.push("its published output");
  const vcaNames = (m.vcas || []).filter((v) => v.members.includes(id)).map((v) => v.name);
  if (vcaNames.length) out.push(`VCA ${vcaNames.join(", ")}`);
  const groupNames = (m.mute_groups || []).filter((g) => g.members.includes(id)).map((g) => g.name);
  if (groupNames.length) out.push(`mute group ${groupNames.join(", ")}`);
  const layoutNames = (m.layouts || []).filter((l) => l.pages.some((p) => p.signals.includes(id))).map((l) => l.name);
  if (layoutNames.length) out.push(`layout ${layoutNames.join(", ")}`);
  if (automixOf(id) || (m.ducks || []).some((d) => d.keys.includes(id) || d.targets.includes(id)) || (m.autolevels || []).some((l) => l.signal === id))
    out.push("the auto setup");
  return out;
});
const logicRefs = computed(() => {
  const id = s.value.id;
  const m = live.mix || {};
  return [...(m.logic || []), ...(m.logic_macros || [])].flatMap((g) => g.blocks || []).filter((b) => b.signal === id).length;
});
async function remove() {
  const id = s.value.id;
  if (await act(() => api.del(signalPath(id)), `${kind.value === "bus" ? "Bus" : "Strip"} ${s.value.name} deleted.`)) {
    dialog.value = "";
    if (live.selected === id) select("");
  }
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

// The input patch, linked (C3): only the first channel is chosen, the others follow in order on the same
// input. Remembered per strip in this browser; without a choice, linked while the patch is in order.
const linkKey = computed(() => `console.link.${s.value.id}`);
const linkChoice = ref(stored(linkKey.value, ""));
watch(linkKey, (k) => (linkChoice.value = stored(k, "")));
const linked = computed(() => s.value.patch.length > 1 && (linkChoice.value ? linkChoice.value === "1" : isLinked(s.value.patch)));
function setLinked(on) {
  linkChoice.value = on ? "1" : "0";
  store(linkKey.value, linkChoice.value);
}
function setLinkedFirst(value) {
  const n = s.value.patch.length;
  if (value.startsWith("t:")) {
    // A talkback mic is mono: on the first channel only.
    patchSignal(s.value.id, { patch: [{ talkback: value.slice(2) }, ...Array(n - 1).fill(null)] });
    return;
  }
  const [input, channel] = value ? value.split(":").map(Number) : [];
  patchSignal(s.value.id, { patch: linkedPatch(n, value ? { input, channel } : null) });
}
const labels = computed(() => channelLabels(s.value.format));
const chLabel = (ch) => labels.value[ch] || String(ch + 1);
const patchText = (p) => (!p ? "—" : "talkback" in p ? `TB ${p.talkback}` : `${p.input}·${p.channel}`);

function togglePolarity(ch) {
  const polarity = [...s.value.polarity];
  polarity[ch] = !polarity[ch];
  patchSignal(s.value.id, { polarity });
}
// Phase on wider strips (C5): one button, a dialog with a toggle per channel; the button shows "some".
const inverted = computed(() => s.value.polarity.filter(Boolean).length);
const setAllPolarity = (on) => patchSignal(s.value.id, { polarity: s.value.polarity.map(() => on) });

const nodeGr = (n) => meters.value?.nodes?.[n.id]?.gain_reduction_db || 0;
const busName = (id) => signalOf(id)?.name || id;
const sendDb = (x) => {
  const v = dbValue(x.gain_db);
  return Number.isFinite(v) ? Math.max(-60, v) : -60;
};
const setSend = (x, v) => patchSend(x.from, x.to, { gain_db: v <= -60 ? "-inf" : v });
const toggleSendMute = (x) => patchSend(x.from, x.to, { mute: !x.mute });
const fmtSend = (v) => (v <= -60 ? "−∞" : fmtSigned(v));
</script>

<template>
  <section
    class="strip"
    :class="{ bus, compact, selected: selected && !compact, muted: s.mute || !!otherMute, onair: tally?.on_air, preview: tally?.preview && !tally?.on_air }"
    :style="{ '--colour': colour || 'var(--border)' }"
    :data-signal="s.id"
  >
    <div class="shead">
      <input
        v-if="editing"
        ref="nameInput"
        v-model="draft"
        class="nmedit"
        maxlength="64"
        :aria-label="`${s.name} name`"
        @keydown.enter.prevent="endRename(true)"
        @keydown.escape.prevent="endRename(false)"
        @blur="endRename(true)"
      />
      <button v-else class="sname" :title="compact ? `${s.name} (${s.id})` : `${s.name} (${s.id}); click to rename`" @click="startRename">
        <span class="nm">{{ s.name }}</span>
        <span class="fmt"
          >{{ s.format }}<span v-if="bus && s.subgroup" class="sub" title="subgroup: its sends are post-fader at 0 dB"> SUB</span><span v-if="bus && s.minus" class="minus" :title="`mix-minus: ${busName(s.minus.of)} without ${s.minus.without.map((w) => signalOf(w)?.name || w).join(', ')}`"> −{{ busName(s.minus.of) }}</span><span v-if="s.recall_safe" class="safe" title="recall-safe"> S</span
          ><span v-if="tally?.afv" class="afv" :class="{ open: tally.afv_open }" :title="`audio follows video: tally ${tally.index}, ${tally.afv_open ? 'open' : 'closed'}`"> AFV</span
          ><span v-if="vad" class="vad" :class="{ on: vad.active }" :title="`voice activity: ${vad.active ? 'speech' : 'no speech'}, probability ${vad.probability.toFixed(2)}`" :data-vad="vad.active ? 'speech' : 'quiet'"> ●</span></span
        >
      </button>
      <div v-if="!compact" ref="menuEl" class="sicons">
        <button class="sicon gear" :aria-label="`${s.name} processing`" title="open its processing" @click="open">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M19.4 13a7.5 7.5 0 0 0 0-2l2.1-1.6-2-3.5-2.5 1a7.4 7.4 0 0 0-1.7-1L15 3.3h-4l-.4 2.6a7.4 7.4 0 0 0-1.7 1l-2.5-1-2 3.5L4.6 11a7.5 7.5 0 0 0 0 2l-2.1 1.6 2 3.5 2.5-1a7.4 7.4 0 0 0 1.7 1l.4 2.6h4l.4-2.6a7.4 7.4 0 0 0 1.7-1l2.5 1 2-3.5zM12 15.5a3.5 3.5 0 1 1 0-7 3.5 3.5 0 0 1 0 7z" /></svg>
        </button>
        <button class="sicon" :aria-label="`${s.name} menu`" :aria-expanded="menu" title="routing, delete" @click="menu = !menu">
          <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="5" r="2" /><circle cx="12" cy="12" r="2" /><circle cx="12" cy="19" r="2" /></svg>
        </button>
        <div v-if="menu" class="smenu" role="menu">
          <button role="menuitem" @click="choose('processing')">Processing…</button>
          <!-- TODO(1.4.0, after PR #38): a "Format…" item opening agent B's FormatSelect.vue (C10). -->
          <button role="menuitem" @click="choose('send-to')">Send to…</button>
          <button v-if="bus" role="menuitem" @click="choose('add-source')">Add source…</button>
          <button v-if="bus" role="menuitem" @click="choose('subgroup')">{{ s.subgroup ? "Plain bus…" : "Subgroup…" }}</button>
          <button role="menuitem" class="danger" @click="choose('delete')">Delete…</button>
        </div>
      </div>
    </div>

    <div v-if="compact" class="grlist">
      <span v-for="n in dynamics" :key="n.id" class="node" :title="`${n.id}: gain reduction ${nodeGr(n).toFixed(1)} dB`">
        <span class="nid">{{ n.id }}</span>
        <span class="gr"><span :style="{ width: `${Math.min(100, (nodeGr(n) / 20) * 100)}%` }"></span></span>
      </span>
    </div>
    <div v-else class="upper">
    <div v-if="!bus" class="patch" :class="{ linked }">
      <label v-if="s.patch.length > 1" class="check linkck" title="choose the first channel; the others follow in order"
        ><input type="checkbox" :checked="linked" :aria-label="`${s.name} link channels`" @change="setLinked($event.target.checked)" /> Link</label
      >
      <template v-if="linked">
        <select
          :class="patchState(0)"
          :value="patchValue(s.patch[0])"
          :aria-label="`${s.name} input`"
          :title="`from ${chLabel(0)}; the others follow`"
          @change="setLinkedFirst($event.target.value)"
        >
          <option value="">—</option>
          <option v-for="o in patchOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
          <option v-for="o in talkOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
        </select>
        <span v-for="(p, ch) in s.patch.slice(1)" :key="ch" class="pchip" :class="patchState(ch + 1)" :title="`${chLabel(ch + 1)}: ${patchState(ch + 1)}`">{{ chLabel(ch + 1) }} {{ patchText(p) }}</span>
      </template>
      <template v-else>
        <select
          v-for="(p, ch) in s.patch"
          :key="ch"
          :class="patchState(ch)"
          :value="patchValue(p)"
          :aria-label="`${s.name} channel ${ch + 1} input`"
          :title="`channel ${ch + 1} (${chLabel(ch)}): ${patchState(ch)}`"
          @change="setPatch(ch, $event.target.value)"
        >
          <option value="">—</option>
          <option v-for="o in patchOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
          <option v-for="o in talkOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
        </select>
      </template>
    </div>

    <template v-if="!bus">
      <MiniPot label="Trim" :value="s.trim_db" :min="-42" :max="42" :step="0.5" :format="(v) => fmtSigned(v)" @change="(v) => patchSignal(s.id, { trim_db: v })" />
      <div class="polarity">
        <button
          v-if="s.polarity.length === 1"
          class="btn small"
          :class="s.polarity[0] ? 'bypass' : 'secondary'"
          :aria-pressed="s.polarity[0]"
          :aria-label="`${s.name} channel 1 polarity`"
          @click="togglePolarity(0)"
        >
          Ø
        </button>
        <button
          v-else
          class="btn small"
          :class="inverted ? ['bypass', { partial: inverted < s.polarity.length }] : 'secondary'"
          :aria-pressed="inverted === 0 ? 'false' : inverted === s.polarity.length ? 'true' : 'mixed'"
          :aria-label="`${s.name} polarity`"
          :title="inverted ? `polarity inverted on ${inverted} of ${s.polarity.length} channels` : 'polarity: per channel'"
          @click="dialog = 'polarity'"
        >
          Ø{{ inverted && inverted < s.polarity.length ? ` ${inverted}/${s.polarity.length}` : "" }}
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
      <template v-for="x in mySends" :key="x.to">
        <button
          v-if="isSubgroup(x.to)"
          class="sgsend"
          :class="{ off: x.mute }"
          :data-subgroup-send="x.to"
          :aria-pressed="!x.mute"
          :title="`send to subgroup ${busName(x.to)}: post-fader at 0 dB, locked; click to ${x.mute ? 'turn on' : 'turn off'}`"
          @click="toggleSendMute(x)"
        >
          <span>{{ busName(x.to) }}</span
          ><span class="num"
            >0.0 <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 10V7a5 5 0 0 1 10 0v3h1v11H6V10zm2 0h6V7a3 3 0 0 0-6 0z" /></svg></span
          >
        </button>
        <MiniPot v-else :label="busName(x.to)" :value="sendDb(x)" :min="-60" :max="10" :step="0.5" :def="0" :format="fmtSend" :title="`send to ${busName(x.to)}`" :class="{ off: x.mute }" @change="(v) => setSend(x, v)" />
      </template>
    </div>

    <template v-if="!bus && (s.format === 'mono' || s.format === 'stereo')">
      <div v-if="surroundPan" class="minipot surroundpan" :title="`the surround panner ${surroundPan.id} pans this strip; ${s.format === 'stereo' ? 'balance' : 'pan'} has no effect`">
        <span class="mp-head"><span>{{ s.format === "stereo" ? "Bal" : "Pan" }}</span><span class="num">surround</span></span>
      </div>
      <MiniPot v-else :label="s.format === 'stereo' ? 'Bal' : 'Pan'" :value="s.pan" :min="-1" :max="1" :step="0.01" :def="0" :format="fmtPan" @change="(v) => patchSignal(s.id, { pan: v })" />
      <MiniPot v-if="s.format === 'stereo'" label="Width" :value="s.width" :min="0" :max="2" :step="0.01" :def="1" :format="(v) => `${Math.round(v * 100)}%`" @change="(v) => patchSignal(s.id, { width: v })" />
    </template>
    </div>

    <div class="solo">
      <button class="btn small" :class="soloed[s.id] === 'pfl' ? 'pfl' : 'secondary'" :aria-pressed="soloed[s.id] === 'pfl'" :aria-label="`PFL ${s.name}`" title="pre-fader listen" @click="toggleSolo(s.id, 'pfl')">PFL</button>
      <button v-if="!compact" class="btn small" :class="soloed[s.id] === 'afl' ? 'pfl' : 'secondary'" :aria-pressed="soloed[s.id] === 'afl'" :aria-label="`AFL ${s.name}`" title="after-fader listen" @click="toggleSolo(s.id, 'afl')">AFL</button>
    </div>
    <button class="btn mute" :class="[s.mute ? 'danger' : 'secondary', { othermute: !s.mute && otherMute }]" :aria-pressed="s.mute" :title="otherMute ? `muted by a ${otherMute === 'VCA' ? 'VCA' : 'mute group'}` : ''" @click="patchSignal(s.id, { mute: !s.mute })">Mute</button>
    <div v-if="auto !== undefined" class="autogain small" :title="autoTitle" :data-auto="Number.isFinite(auto) ? auto.toFixed(1) : '-inf'">
      <span class="gr"><span :style="{ width: `${Math.min(100, (-Math.max(auto, -30) / 30) * 100)}%` }"></span></span>
      <span>{{ Number.isFinite(auto) ? fmtSigned(auto) : "−∞" }}</span>
    </div>
    <div v-if="myVcas.length && !compact" class="vcachips muted small" :title="`VCAs: ${myVcas.map((v) => v.name).join(', ')}`">
      <span v-for="v in myVcas" :key="v.id" class="vcachip" :style="{ borderColor: v.colour || 'var(--border)' }">{{ v.name }}</span>
    </div>

    <div class="faderrow">
      <VFader :value="s.fader_db" :label="`${s.name} fader`" :colour="colour" @change="(v) => patchSignal(s.id, { fader_db: v })" />
      <Meter :signal="s.id" :values="meters" :channels="channels" :scale="scale" :alignment="alignment" :marks="false" />
    </div>
    <div v-if="!compact" class="sfoot muted small" :title="`processing latency of ${s.name} at its post-fader point`">{{ latency ? `${latency.toFixed(2)} ms` : " " }}</div>

    <Modal v-if="dialog === 'polarity'" :title="`Polarity: ${s.name}`" @close="dialog = ''">
      <div class="poltoggles">
        <button
          v-for="(p, ch) in s.polarity"
          :key="ch"
          class="btn"
          :class="p ? 'bypass' : 'secondary'"
          :aria-pressed="p"
          :aria-label="`${s.name} ${chLabel(ch)} polarity`"
          @click="togglePolarity(ch)"
        >
          Ø {{ chLabel(ch) }}
        </button>
      </div>
      <div class="actions">
        <button class="btn secondary" @click="setAllPolarity(false)">None</button>
        <button class="btn secondary" @click="setAllPolarity(true)">All</button>
        <button class="btn" @click="dialog = ''">Done</button>
      </div>
    </Modal>
    <Modal v-if="dialog === 'send-to'" :title="`Send ${s.name} to…`" @close="dialog = ''">
      <p class="note">A new send at 0 dB from {{ bus ? "the bus after its fader" : "the strip after its fader and pan" }}; change its level and tap on the strip or in Routing.</p>
      <div class="pickgrid">
        <button v-for="b in targets" :key="b.id" class="btn secondary" :data-pick="b.id" @click="route(s.id, b.id)">{{ b.name }}<span v-if="b.subgroup" class="muted small"> SUB</span></button>
      </div>
      <div v-if="!targets.length" class="empty">No bus left to send to.</div>
    </Modal>
    <Modal v-if="dialog === 'add-source'" :title="`Add a source to ${s.name}`" @close="dialog = ''">
      <p class="note">A new send into {{ s.name }} at 0 dB from the source's output (a strip after its fader and pan, a bus after its fader).</p>
      <div class="pickgrid">
        <button v-for="x in sources" :key="x.id" class="btn secondary" :data-pick="x.id" @click="route(x.id, s.id)">{{ x.name }}</button>
      </div>
      <div v-if="!sources.length" class="empty">Every strip and bus already sends here.</div>
    </Modal>
    <Modal v-if="dialog === 'subgroup'" :title="s.subgroup ? `${s.name}: plain bus` : `${s.name}: subgroup`" @close="dialog = ''">
      <template v-if="!s.subgroup">
        <p>Every send into {{ s.name }} is then taken after its source's fader and pan at 0 dB, and its level is locked: the bus fader is the group level.</p>
        <p v-if="relevelled" class="warnbox">{{ relevelled }} of its {{ intoMe.length }} sends change their level or tap now (ramped).</p>
      </template>
      <p v-else>Its sends stay at 0 dB after the fader, and their level and tap can be changed again.</p>
      <div class="actions">
        <button class="btn secondary" @click="dialog = ''">Cancel</button>
        <button class="btn" @click="setSubgroup(!s.subgroup)">{{ s.subgroup ? "Make it a plain bus" : "Make it a subgroup" }}</button>
      </div>
    </Modal>
    <Modal v-if="dialog === 'delete'" :title="`Delete ${kind} ${s.name}?`" @close="dialog = ''">
      <p>{{ s.name }} ({{ s.id }}) is removed from the mix at once<template v-if="refs.length">, with {{ refs.join("; ") }}</template>.</p>
      <p v-if="logicRefs" class="warnbox">{{ logicRefs }} logic block{{ logicRefs === 1 ? "" : "s" }} name{{ logicRefs === 1 ? "s" : "" }} it: they stay with no target and stop acting.</p>
      <p class="note">This cannot be undone; snapshots keep their values but not the {{ kind }}.</p>
      <div class="actions">
        <button class="btn secondary" @click="dialog = ''">Cancel</button>
        <button class="btn danger" @click="remove">Delete {{ s.name }}</button>
      </div>
    </Modal>
  </section>
</template>
