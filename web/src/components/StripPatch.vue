<script setup>
// A strip's input patch (A1). The strip shows channel 1's source only: a mono strip picks it right there;
// a wider strip's button opens a dialog with every channel's source side by side, and the Link switch
// (C3): linked, only the first channel is chosen and the others follow in order on the same input.
// Linked is remembered per strip in this browser; without a choice, linked while the patch is in order.
import { computed, ref, watch } from "vue";
import Modal from "./Modal.vue";
import { channelLabels, isLinked, linkedPatch } from "../console.js";
import { signalPatchKey, signalPatchOf, signalPatchText, signalSources } from "../patchsources.js";
import { live, patchSignal, store, stored } from "../store.js";

const props = defineProps({ signal: { type: Object, required: true } });
const s = computed(() => props.signal);
const open = ref(false);

const inputs = computed(() => live.status?.inputs || []);
function patchValue(p) {
  if (!p) return "";
  if ("signal" in p || "output" in p) return signalPatchKey(p);
  return "talkback" in p ? `t:${p.talkback}` : `${p.input}:${p.channel}`;
}
function setPatch(ch, value) {
  const patch = [...s.value.patch];
  if (!value) patch[ch] = null;
  else if (signalPatchOf(value)) patch[ch] = signalPatchOf(value);
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
// Buses, direct outs and virtual outputs as inputs (D1); a loop is greyed out and refused by the server.
const signalGroups = computed(() => signalSources(s.value.id));
const patchState = (ch) => s.value.patch_state?.[ch] || (s.value.patch?.[ch] ? "ok" : "unpatched");
// The worst state of all channels, for the strip's button.
const overall = computed(() => (s.value.patch.some((_, ch) => patchState(ch) === "missing") ? "missing" : patchState(0)));

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
  const point = signalPatchOf(value);
  if (point) {
    patchSignal(s.value.id, { patch: linkedPatch(n, point) });
    return;
  }
  const [input, channel] = value ? value.split(":").map(Number) : [];
  patchSignal(s.value.id, { patch: linkedPatch(n, value ? { input, channel } : null) });
}
function pick(ch, value) {
  if (ch === 0 && linked.value) setLinkedFirst(value);
  else setPatch(ch, value);
}
const labels = computed(() => channelLabels(s.value.format));
const chLabel = (ch) => labels.value[ch] || String(ch + 1);
const patchText = (p) => (!p ? "—" : "talkback" in p ? `TB ${p.talkback}` : "input" in p ? `${p.input}·${p.channel}` : signalPatchText(p));
</script>

<template>
  <div class="patch">
    <select
      v-if="s.patch.length === 1"
      :class="patchState(0)"
      :value="patchValue(s.patch[0])"
      :aria-label="`${s.name} input`"
      :title="`input: ${patchState(0)}`"
      @change="setPatch(0, $event.target.value)"
    >
      <option value="">—</option>
      <option v-for="o in patchOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
      <option v-for="o in talkOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
      <optgroup v-for="g in signalGroups" :key="g.label" :label="g.label">
        <option v-for="o in g.points" :key="o.value" :value="o.value" :disabled="o.disabled" :title="o.title">{{ o.text }}</option>
      </optgroup>
    </select>
    <button
      v-else
      class="patchbtn"
      :class="overall"
      :aria-label="`${s.name} input`"
      :title="`${chLabel(0)} from ${patchText(s.patch[0])}${linked ? '; the others follow' : ''}: click for every channel`"
      @click="open = true"
    >
      <span>{{ patchText(s.patch[0]) }}</span><span class="more">{{ linked ? "→" : "…" }}</span>
    </button>

    <Modal v-if="open" :title="`Input: ${s.name}`" wide @close="open = false">
      <label class="check linkck" title="choose the first channel; the others follow in order"
        ><input type="checkbox" :checked="linked" :aria-label="`${s.name} link channels`" @change="setLinked($event.target.checked)" /> Link: the first channel chooses, the others follow in order</label
      >
      <div class="patchgrid">
        <label v-for="(p, ch) in s.patch" :key="ch" class="patchcell" :class="{ follows: linked && ch > 0 }">
          <span class="chl">{{ chLabel(ch) }}</span>
          <select
            :class="patchState(ch)"
            :value="patchValue(p)"
            :disabled="linked && ch > 0"
            :aria-label="`${s.name} channel ${ch + 1} input`"
            :title="`channel ${ch + 1} (${chLabel(ch)}): ${patchState(ch)}`"
            @change="pick(ch, $event.target.value)"
          >
            <option value="">—</option>
            <option v-for="o in patchOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
            <option v-for="o in talkOptions" :key="o.value" :value="o.value">{{ o.text }}</option>
            <optgroup v-for="g in signalGroups" :key="g.label" :label="g.label">
              <option v-for="o in g.points" :key="o.value" :value="o.value" :disabled="o.disabled" :title="o.title">{{ o.text }}</option>
            </optgroup>
          </select>
        </label>
      </div>
      <div class="actions"><button class="btn secondary" @click="open = false">Close</button></div>
    </Modal>
  </div>
</template>
