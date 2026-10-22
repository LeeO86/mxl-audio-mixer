<script setup>
// A strip's or bus's own settings (§10.3, §12.1): name, colour, recall-safe, loudness; for strips the tally
// index and audio-follows-video (open while the index is on air, at its depth otherwise, with fade times).
import { computed, ref, watch } from "vue";
import Pot from "./Pot.vue";
import DownmixEdit from "./DownmixEdit.vue";
import ConfirmButton from "./ConfirmButton.vue";
import { DEFAULT_DOWNMIX } from "../nodes.js";
import { dbValue } from "../api.js";
import { buses, isBus, live, patchSignal, signalOf, signals, tallyOf } from "../store.js";

const props = defineProps({ signal: { type: Object, required: true } });
const s = computed(() => props.signal);
const bus = computed(() => isBus(s.value.id));
const tally = computed(() => tallyOf(s.value.id));
const tslEnabled = computed(() => !!live.status?.tally?.enabled);

const name = ref("");
watch(
  () => s.value.name,
  (n) => (name.value = n),
  { immediate: true },
);
const set = (body) => patchSignal(s.value.id, body);
// A bus as a mix-minus (§5): every send into `of` except those from `without`.
const otherBuses = computed(() => buses.value.filter((b) => b.id !== s.value.id));
const setMinus = (of) => set({ minus: of ? { of, without: s.value.minus?.without || [] } : null });
const toggleWithout = (id, on) => {
  const w = s.value.minus?.without || [];
  set({ minus: { of: s.value.minus.of, without: on ? [...w, id] : w.filter((x) => x !== id) } });
};
const minusSources = computed(() => {
  const of = s.value.minus?.of;
  return of ? (live.mix?.sends || []).filter((x) => x.to === of).map((x) => x.from) : [];
});
// A bus's own down-mix of everything arriving from a wider format (D2); off: the mix's.
const globalDownmix = computed(() => live.mix?.settings?.downmix || DEFAULT_DOWNMIX);
const setOwnDownmix = (on) => set({ downmix: on ? { ...globalDownmix.value } : null });
function saveName() {
  if (name.value.trim() && name.value !== s.value.name) set({ name: name.value.trim() });
}

// The defaults when AFV is first switched on (IMPLEMENTATION_PLAN.md, widgets and tally, D57).
const AFV_DEFAULTS = { enabled: true, depth_db: "-inf", fade_in_ms: 100, fade_out_ms: 500 };
const afv = computed(() => s.value.afv || null);
function setAfv(change) {
  set({ afv: { ...(afv.value || AFV_DEFAULTS), ...change } });
}
function setIndex(v) {
  set({ tally_index: v === "" ? null : Math.max(0, Math.min(65535, Math.round(Number(v)))) });
}
const depth = computed(() => {
  const d = dbValue(afv.value?.depth_db);
  return Number.isFinite(d) ? Math.max(-80, d) : -80;
});
</script>

<template>
  <div class="panel">
    <h3>{{ s.name }} <span class="muted" style="text-transform: none; letter-spacing: 0">{{ s.id }} · {{ s.format }}</span></h3>
    <div class="params">
      <div>
        <label>Name</label>
        <input v-model="name" aria-label="Name" @change="saveName" @keydown.enter="saveName" />
      </div>
      <div>
        <label>Colour</label>
        <div class="row tight">
          <input type="color" :value="s.colour || '#8496a8'" aria-label="Colour" style="width: 3rem; padding: 0.1rem" @change="set({ colour: $event.target.value })" />
          <button class="btn small secondary" :disabled="!s.colour" @click="set({ colour: '' })">No colour</button>
        </div>
        <label class="check"><input type="checkbox" :checked="s.recall_safe" @change="set({ recall_safe: $event.target.checked })" /> Recall-safe</label>
        <label class="check" title="solo in place never mutes it"><input type="checkbox" :checked="s.solo_safe" @change="set({ solo_safe: $event.target.checked })" /> Solo-safe</label>
        <label class="check" title="mute groups never mute it (isolate)"><input type="checkbox" :checked="s.mute_safe" @change="set({ mute_safe: $event.target.checked })" /> Mute-safe</label>
        <label class="check"><input type="checkbox" :checked="s.loudness" @change="set({ loudness: $event.target.checked })" /> Measure loudness</label>
      </div>
      <div v-if="bus" class="minusedit">
        <label>Mix-minus of</label>
        <select :value="s.minus?.of || ''" aria-label="Mix-minus of" @change="setMinus($event.target.value)">
          <option value="">— (a plain bus)</option>
          <option v-for="b in otherBuses" :key="b.id" :value="b.id">{{ b.name }}</option>
        </select>
        <template v-if="s.minus">
          <p class="note">Takes every send into {{ signalOf(s.minus.of)?.name || s.minus.of }}, as it is, except:</p>
          <label v-for="id in minusSources" :key="id" class="check"><input type="checkbox" :checked="s.minus.without.includes(id)" :aria-label="`without ${signalOf(id)?.name || id}`" @change="toggleWithout(id, $event.target.checked)" /> {{ signalOf(id)?.name || id }}</label>
        </template>
      </div>
      <div v-if="bus" class="downmixedit">
        <label>Down-mix of what arrives here</label>
        <template v-if="s.downmix">
          <DownmixEdit :value="s.downmix" label="Bus down-mix" @change="(d) => set({ downmix: d })" />
          <ConfirmButton small label="Use the mix's down-mix" aria-label="Use the mix's down-mix" @confirm="setOwnDownmix(false)" />
          <p class="note">Going back to the mix's fades the affected sends out and in: a brief dip.</p>
        </template>
        <template v-else>
          <p class="note">The mix's down-mix (Settings).</p>
          <button class="btn small secondary" aria-label="Own down-mix" @click="setOwnDownmix(true)">Own down-mix</button>
        </template>
      </div>
      <template v-if="!bus">
        <div>
          <label>Tally index (TSL)</label>
          <input type="number" min="0" max="65535" :value="s.tally_index ?? ''" placeholder="none" aria-label="Tally index" @change="setIndex($event.target.value)" />
          <p class="note">
            <template v-if="!tslEnabled">TSL is off (TSL_ENABLE).</template>
            <template v-else-if="tally">
              <span class="lamp" :class="{ c1: tally.on_air }"></span> on air · <span class="lamp" :class="{ c2: tally.preview }"></span> preview
            </template>
          </p>
        </div>
        <div>
          <label class="check" style="margin-top: 0.5rem">
            <input type="checkbox" :checked="!!afv?.enabled" :disabled="s.tally_index === undefined || s.tally_index === null" @change="setAfv({ enabled: $event.target.checked })" />
            Audio follows video
          </label>
          <p v-if="s.tally_index === undefined || s.tally_index === null" class="note">Set a tally index first.</p>
          <p v-else-if="afv?.enabled" class="note">{{ tally?.afv_open ? "Open: the index is on air." : "Closed: the index is not on air." }}</p>
        </div>
        <template v-if="afv">
          <Pot label="AFV depth" unit="dB (−80: −∞)" :value="depth" :min="-80" :max="0" :def="-80" :fine="1" @change="(v) => setAfv({ depth_db: v <= -80 ? '-inf' : v })" />
          <Pot label="AFV fade in" unit="ms" :value="afv.fade_in_ms" :min="0" :max="10000" :def="100" :fine="10" :digits="0" @change="(v) => setAfv({ fade_in_ms: v })" />
          <Pot label="AFV fade out" unit="ms" :value="afv.fade_out_ms" :min="0" :max="10000" :def="500" :fine="10" :digits="0" @change="(v) => setAfv({ fade_out_ms: v })" />
        </template>
      </template>
    </div>
  </div>
</template>
