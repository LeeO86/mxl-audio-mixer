<script setup>
// A down-mix (D1/D2): how a wider format folds into a narrower one, ITU-R BS.775 style. The centre and
// the surrounds into the front pair at their levels, the LFE into the front pair (off by default), and
// normalisation (no output channel above unity). Levels at the bottom of their range are −∞ (off).
// Edits are a draft until Apply: a new down-mix rewires the affected sends with a short fade.
import { computed, ref, watch } from "vue";
import Pot from "./Pot.vue";
import { dbValue } from "../api.js";

const props = defineProps({
  value: { type: Object, required: true }, // { centre_db, surround_db, lfe_db, normalise }
  label: { type: String, default: "Down-mix" },
});
const emit = defineEmits(["change"]);
const OFF = -41;
const DEFAULT = { centre_db: -3, surround_db: -3, lfe_db: "-inf", normalise: false };
const draft = ref({ ...props.value });
watch(
  () => JSON.stringify(props.value),
  () => (draft.value = { ...props.value }),
);
const shown = (v) => {
  const d = dbValue(v);
  return Number.isFinite(d) ? d : OFF;
};
const set = (key, v) => (draft.value = { ...draft.value, [key]: typeof v === "number" && v < -40 ? "-inf" : v });
const same = (a, b) => ["centre_db", "surround_db", "lfe_db"].every((k) => dbValue(a[k]) === dbValue(b[k])) && !!a.normalise === !!b.normalise;
const dirty = computed(() => !same(draft.value, props.value));
</script>

<template>
  <div class="downmix">
    <Pot :label="`${label}: centre`" unit="dB (−41: off)" :value="shown(draft.centre_db)" :min="OFF" :max="0" :def="-3" :fine="0.5" @change="(v) => set('centre_db', v)" />
    <Pot :label="`${label}: surrounds`" unit="dB (−41: off)" :value="shown(draft.surround_db)" :min="OFF" :max="0" :def="-3" :fine="0.5" @change="(v) => set('surround_db', v)" />
    <Pot :label="`${label}: LFE`" unit="dB (−41: off)" :value="shown(draft.lfe_db)" :min="OFF" :max="10" :def="OFF" :fine="0.5" @change="(v) => set('lfe_db', v)" />
    <label class="check"><input type="checkbox" :checked="draft.normalise" :aria-label="`${label}: normalise`" @change="set('normalise', $event.target.checked)" /> Normalise (no channel above unity)</label>
    <div class="row tight">
      <button class="btn small" :disabled="!dirty" :aria-label="`${label}: apply`" @click="emit('change', { ...draft })">Apply</button>
      <button class="btn small secondary" :disabled="!dirty" @click="draft = { ...value }">Undo</button>
      <button class="btn small secondary" :disabled="same(draft, DEFAULT)" title="ITU-R BS.775: −3 dB, −3 dB, LFE off" @click="draft = { ...DEFAULT }">BS.775</button>
    </div>
  </div>
</template>
