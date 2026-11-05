<script setup>
// One console strip (§12.1): name and colour, input patch, trim, polarity, the chain overview (response
// curve, nodes with gain reduction), sends, pan and width, mute, the fader and the meter. Buses have no
// patch, trim or pan. Every change goes to the API; the strip follows the mix pushed back. `compact` (the
// strip and fader-bank widgets) keeps the name, gain reduction, mute, fader and meter. The name shows the
// tally of the strip's index: red on air, green on preview. A click on the name renames the strip (C1);
// the gear opens its processing, the ⋮ menu routes it (C11), makes a bus a subgroup (C12) or deletes it
// (C9).
//
// 1.6.0 (A1–A13): the name sits at the bottom (two lines, then … with the full name as its tooltip); the
// input shows channel 1's source (StripPatch: every channel in a dialog); the L/R pan sits right above
// PFL/AFL with the surround panner in a dialog (StripPan); Select sits beside Mute; a strip of an auto-mix
// carries the auto-mix's colour mark, and a click on its auto-mix meter takes it out of the sharing (grey)
// and back; a strip under VCAs shows a grey "VCA" cap where its fader is with them; a strip of two or more
// channels unfolds (the triangle) into a fader per channel (StripMembers).
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import Meter from "./Meter.vue";
import MiniCurve from "./MiniCurve.vue";
import MiniPot from "./MiniPot.vue";
import FormatSelect from "./FormatSelect.vue";
import Modal from "./Modal.vue";
import StripMembers from "./StripMembers.vue";
import StripPan from "./StripPan.vue";
import StripPatch from "./StripPatch.vue";
import VFader from "./VFader.vue";
import { api, dbValue, fmtSigned } from "../api.js";
import { channelLabels } from "../console.js";
import { DYNAMICS } from "../nodes.js";
import {
  act,
  addSend,
  buses,
  autoGain,
  automixOf,
  automixes,
  groupMuted,
  isBus,
  isSubgroup,
  live,
  patchAutomix,
  patchSend,
  patchSignal,
  select,
  sendSources,
  sendTargets,
  sends,
  signalOf,
  signalPath,
  soloed,
  stored,
  store,
  strips,
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
const dynamics = computed(() => s.value.chain.filter((n) => DYNAMICS.has(n.type)));
const vad = computed(() => live.vad[s.value.id] || null);
const auto = computed(() => autoGain(s.value.id));
const myAutomix = computed(() => automixOf(s.value.id));
const myMember = computed(() => myAutomix.value?.members.find((m) => m.signal === s.value.id) || null);
const autoTitle = computed(() => {
  const am = myAutomix.value;
  const g = Number.isFinite(auto.value) ? `${auto.value.toFixed(1)} dB` : "−∞";
  if (!am) return `auto gain ${g} (ducking), after the fader`;
  return myMember.value?.bypass
    ? `auto-mix ${am.name}: out of the sharing (0 dB); click to take part again`
    : `auto gain ${g} (auto-mix ${am.name}), after the fader; click to take this strip out of the sharing`;
});
// The auto-mix colour mark (A9): a fixed colour per auto-mix, by its place in the list.
const AUTOMIX_COLOURS = ["#e08a3a", "#3aa0e0", "#b45ad8", "#4dbf6a", "#d8c23a", "#e05a7a", "#3ac2b0", "#8a8ae0"];
const automixColour = computed(() => {
  const i = automixes.value.findIndex((a) => a.id === myAutomix.value?.id);
  return i < 0 ? "" : AUTOMIX_COLOURS[i % AUTOMIX_COLOURS.length];
});
// A click on the auto-mix meter takes the strip out of the sharing and back (A11): its member's bypass.
function toggleAutomix() {
  const am = myAutomix.value;
  if (!am || props.compact) return;
  const on = !myMember.value?.bypass;
  patchAutomix(am.id, { members: am.members.map((m) => (m.signal === s.value.id ? { ...m, bypass: on } : m)) });
}
// Where the fader is with its VCAs (A10): their offsets added, at most +15 dB.
const vcaGhost = computed(() => {
  if (!myVcas.value.length) return null;
  const total = myVcas.value.reduce((sum, v) => sum + dbValue(v.fader_db), dbValue(s.value.fader_db));
  return Number.isFinite(total) ? Math.round(total * 10) / 10 : "-inf";
});
// Unfolded (A13): a fader per channel; remembered per strip in this browser.
const unfoldKey = computed(() => `console.unfold.${s.value.id}`);
const unfolded = ref(stored(unfoldKey.value, "") === "1");
watch(unfoldKey, (k) => (unfolded.value = stored(k, "") === "1"));
const canUnfold = computed(() => !props.bus && !props.compact && channels.value > 1);
function toggleUnfold() {
  unfolded.value = !unfolded.value;
  store(unfoldKey.value, unfolded.value ? "1" : "");
}
// Select (A12): the strip the processing view edits.
const toggleSelect = () => select(selected.value ? "" : s.value.id);
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
  if (props.compact) return emit("open", s.value.id);
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
const menuBtn = ref(null);
// The menu opens below its button, as a fixed layer kept inside the window: the strips' scroll area
// does not cut it off (A3: the first strip's menu was covered at the left edge).
const menuPos = ref({});
const MENU_W = 160;
function toggleMenu() {
  menu.value = !menu.value;
  if (!menu.value) return;
  const r = menuBtn.value?.getBoundingClientRect();
  if (!r) return;
  const left = Math.max(4, Math.min(window.innerWidth - MENU_W - 4, r.right - MENU_W));
  menuPos.value = { position: "fixed", top: `${r.bottom + 2}px`, left: `${left}px`, right: "auto", minWidth: `${MENU_W}px` };
}
const closeMenu = (e) => {
  if (!menuEl.value?.contains(e.target)) menu.value = false;
};
watch(menu, (on) => (on ? window.addEventListener("mousedown", closeMenu) : window.removeEventListener("mousedown", closeMenu)));
onUnmounted(() => window.removeEventListener("mousedown", closeMenu));
const dialog = ref(""); // "send-to", "add-source", "subgroup", "format", "delete", "polarity"
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
// Logic blocks that name it (also a set-monitor choosing it, a set-talk to a talkback destination on it),
// and other signals' keys on it: they stay, emptied (D187).
const logicRefs = computed(() => {
  const id = s.value.id;
  const m = live.mix || {};
  const dests = new Set((m.control_room?.talkback?.destinations || []).filter((d) => d.bus === id).map((d) => d.id));
  return [...(m.logic || []), ...(m.logic_macros || [])]
    .flatMap((g) => g.blocks || [])
    .filter((b) => b.signal === id || b.source?.signal === id || (b.type === "set-talk" && dests.has(b.destination))).length;
});
const keyRefs = computed(() => {
  const id = s.value.id;
  return [...strips.value, ...buses.value].filter((x) => x.id !== id && x.chain.some((n) => n.key?.signal === id)).map((x) => x.name);
});
const talkRefs = computed(() => {
  const t = live.mix?.control_room?.talkback;
  const id = s.value.id;
  return (t?.destinations || []).filter((d) => d.bus === id).map((d) => d.name);
});
async function remove() {
  const id = s.value.id;
  if (await act(() => api.del(signalPath(id)), `${kind.value === "bus" ? "Bus" : "Strip"} ${s.value.name} deleted.`)) {
    dialog.value = "";
    if (live.selected === id) select("");
  }
}

const labels = computed(() => channelLabels(s.value.format));
const chLabel = (ch) => labels.value[ch] || String(ch + 1);

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
// The sends show their level only (A6): the Routing tab changes them.
const toRouting = () => (location.hash = "routing");
const toggleSendMute = (x) => patchSend(x.from, x.to, { mute: !x.mute });
const fmtSend = (v) => (v <= -60 ? "−∞" : fmtSigned(v));
</script>

<template>
  <section
    class="strip"
    :class="{ bus, compact, wide: channels >= 8, unfolded: unfolded && canUnfold, selected: selected && !compact, muted: s.mute || !!otherMute, onair: tally?.on_air, preview: tally?.preview && !tally?.on_air }"
    :style="{ '--colour': colour || 'var(--border)' }"
    :data-signal="s.id"
  >
    <div class="shead">
      <div class="sfmt">
        <button
          v-if="canUnfold"
          class="sicon unfold"
          :class="{ open: unfolded }"
          :aria-expanded="unfolded"
          :aria-label="`${s.name} unfold`"
          :title="unfolded ? 'fold: one fader for the strip' : 'unfold: a fader per channel'"
          @click="toggleUnfold"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5l9 7-9 7z" /></svg>
        </button>
        <span class="fmt"
          >{{ s.format }}<span v-if="bus && s.subgroup" class="sub" title="subgroup: its sends are post-fader at 0 dB"> SUB</span><span v-if="bus && s.minus" class="minus" :title="`mix-minus: ${busName(s.minus.of)} without ${s.minus.without.map((w) => signalOf(w)?.name || w).join(', ')}`"> −{{ busName(s.minus.of) }}</span><span v-if="s.recall_safe" class="safe" title="recall-safe"> S</span
          ><span v-if="tally?.afv" class="afv" :class="{ open: tally.afv_open }" :title="`audio follows video: tally ${tally.index}, ${tally.afv_open ? 'open' : 'closed'}`"> AFV</span
          ><span v-if="vad" class="vad" :class="{ on: vad.active }" :title="`voice activity: ${vad.active ? 'speech' : 'no speech'}, probability ${vad.probability.toFixed(2)}`" :data-vad="vad.active ? 'speech' : 'quiet'"> ●</span></span
        >
        <span v-if="automixColour" class="ammark" :style="{ background: automixColour }" :data-automix="myAutomix.id" :title="`auto-mix ${myAutomix.name}`"></span>
        <div v-if="!compact" ref="menuEl" class="sicons">
          <button class="sicon gear" :aria-label="`${s.name} processing`" title="open its processing" @click="open">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M19.4 13a7.5 7.5 0 0 0 0-2l2.1-1.6-2-3.5-2.5 1a7.4 7.4 0 0 0-1.7-1L15 3.3h-4l-.4 2.6a7.4 7.4 0 0 0-1.7 1l-2.5-1-2 3.5L4.6 11a7.5 7.5 0 0 0 0 2l-2.1 1.6 2 3.5 2.5-1a7.4 7.4 0 0 0 1.7 1l.4 2.6h4l.4-2.6a7.4 7.4 0 0 0 1.7-1l2.5 1 2-3.5zM12 15.5a3.5 3.5 0 1 1 0-7 3.5 3.5 0 0 1 0 7z" /></svg>
          </button>
          <button ref="menuBtn" class="sicon" :aria-label="`${s.name} menu`" :aria-expanded="menu" title="routing, delete" @click="toggleMenu">
            <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="5" r="2" /><circle cx="12" cy="12" r="2" /><circle cx="12" cy="19" r="2" /></svg>
          </button>
          <div v-if="menu" class="smenu" role="menu" :style="menuPos">
            <button role="menuitem" @click="choose('processing')">Processing…</button>
            <button role="menuitem" @click="choose('format')">Format…</button>
            <button role="menuitem" @click="choose('send-to')">Send to…</button>
            <button v-if="bus" role="menuitem" @click="choose('add-source')">Add source…</button>
            <button v-if="bus" role="menuitem" @click="choose('subgroup')">{{ s.subgroup ? "Plain bus…" : "Subgroup…" }}</button>
            <button role="menuitem" class="danger" @click="choose('delete')">Delete…</button>
          </div>
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
    <StripPatch v-if="!bus" :signal="s" />

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
        <button v-else class="sendval" :class="{ off: x.mute }" :data-send="x.to" :title="`send to ${busName(x.to)}: ${fmtSend(sendDb(x))} dB, ${x.tap}${x.mute ? ', off' : ''}; change it in Routing`" @click="toRouting">
          <span>{{ busName(x.to) }}</span><span class="num">{{ fmtSend(sendDb(x)) }}</span>
        </button>
      </template>
    </div>
    </div>

    <StripPan v-if="!bus && !compact" :signal="s" />
    <div class="solo">
      <button data-learn="pfl" class="btn small" :class="soloed[s.id] === 'pfl' ? 'pfl' : 'secondary'" :aria-pressed="soloed[s.id] === 'pfl'" :aria-label="`PFL ${s.name}`" title="pre-fader listen" @click="toggleSolo(s.id, 'pfl')">PFL</button>
      <button v-if="!compact" data-learn="afl" class="btn small" :class="soloed[s.id] === 'afl' ? 'pfl' : 'secondary'" :aria-pressed="soloed[s.id] === 'afl'" :aria-label="`AFL ${s.name}`" title="after-fader listen" @click="toggleSolo(s.id, 'afl')">AFL</button>
    </div>
    <div class="muterow">
      <button data-learn="mute" class="btn mute" :class="[s.mute ? 'danger' : 'secondary', { othermute: !s.mute && otherMute }]" :aria-pressed="s.mute" :title="otherMute ? `muted by a ${otherMute === 'VCA' ? 'VCA' : 'mute group'}` : ''" @click="patchSignal(s.id, { mute: !s.mute })">Mute</button>
      <button v-if="!compact" class="btn sel" :class="selected ? '' : 'secondary'" :aria-pressed="selected" :aria-label="`${s.name} select`" title="select: the processing view edits it" @click="toggleSelect">Sel</button>
    </div>
    <button
      v-if="auto !== undefined"
      class="autogain small"
      :class="{ off: myMember?.bypass, clickable: !!myAutomix && !compact }"
      :title="autoTitle"
      :aria-pressed="myAutomix ? !myMember?.bypass : undefined"
      :aria-label="`${s.name} auto gain`"
      :data-auto="Number.isFinite(auto) ? auto.toFixed(1) : '-inf'"
      :style="automixColour ? { '--am': automixColour } : null"
      @click="toggleAutomix"
    >
      <span class="gr"><span :style="{ width: `${Math.min(100, (-Math.max(auto, -30) / 30) * 100)}%` }"></span></span>
      <span>{{ myMember?.bypass ? "off" : Number.isFinite(auto) ? fmtSigned(auto) : "−∞" }}</span>
    </button>
    <div v-if="myVcas.length && !compact" class="vcachips muted small" :title="`VCAs: ${myVcas.map((v) => v.name).join(', ')}`">
      <span v-for="v in myVcas" :key="v.id" class="vcachip" :style="{ borderColor: v.colour || 'var(--border)' }">{{ v.name }}</span>
    </div>

    <StripMembers v-if="unfolded && canUnfold" :signal="s" :meters="meters" :scale="scale" :alignment="alignment" :colour="colour" :muted="s.mute || !!otherMute" />
    <div v-else class="faderrow">
      <VFader data-learn="fader" :value="s.fader_db" :label="`${s.name} fader`" :colour="colour" :ghost="compact ? null : vcaGhost" @change="(v) => patchSignal(s.id, { fader_db: v })" />
      <Meter :signal="s.id" :values="meters" :channels="channels" :labels="labels" :scale="scale" :alignment="alignment" :marks="false" :muted="s.mute || !!otherMute" />
    </div>
    <div v-if="!compact" class="sfoot muted small" :title="`processing latency of ${s.name} at its post-fader point`">{{ latency ? `${latency.toFixed(2)} ms` : " " }}</div>
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
    </button>

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
    <Modal v-if="dialog === 'format'" :title="`Format: ${s.name}`" @close="dialog = ''">
      <FormatSelect :signal="s" actions @close="dialog = ''" />
    </Modal>
    <Modal v-if="dialog === 'delete'" :title="`Delete ${kind} ${s.name}?`" @close="dialog = ''">
      <p>{{ s.name }} ({{ s.id }}) is removed from the mix at once<template v-if="refs.length">, with {{ refs.join("; ") }}</template>.</p>
      <p v-if="talkRefs.length">Talkback destination{{ talkRefs.length === 1 ? "" : "s" }} {{ talkRefs.join(", ") }} go{{ talkRefs.length === 1 ? "es" : "" }} with it.</p>
      <p v-if="keyRefs.length" class="warnbox">The key of {{ keyRefs.join(", ") }} is cleared: a compressor keys itself again, a ducker hears nothing.</p>
      <p v-if="logicRefs" class="warnbox">
        {{ logicRefs }} logic block{{ logicRefs === 1 ? "" : "s" }} name{{ logicRefs === 1 ? "s" : "" }} it: they stay, wired, with no target and no longer act on it (they still pass their value to the blocks after them; a monitor choice gets no source).
      </p>
      <p class="note">This cannot be undone; snapshots keep their values but not the {{ kind }}.</p>
      <div class="actions">
        <button class="btn secondary" @click="dialog = ''">Cancel</button>
        <button class="btn danger" @click="remove">Delete {{ s.name }}</button>
      </div>
    </Modal>
  </section>
</template>
