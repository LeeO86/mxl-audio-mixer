<script setup>
// A VCA (§9): its fader (added to every member's, nested VCAs included), its mute (mutes the members without
// touching their mute buttons), PFL and AFL of all members, and spill: the console shows only its members.
import { computed } from "vue";
import VFader from "./VFader.vue";
import { patchVca, soloed, spill, toggleSolo, vcaSignals, vcasAbove } from "../store.js";

const props = defineProps({
  vca: { type: Object, required: true },
  // In the vca widget: spill toggles the member strips beside it.
  compact: { type: Boolean, default: false },
});
const emit = defineEmits(["spill"]);
const v = computed(() => props.vca);
const members = computed(() => vcaSignals(v.value.id));
const above = computed(() => vcasAbove(v.value.id));
const spilled = computed(() => spill.vca === v.value.id);
function toggleSpill() {
  spill.vca = spilled.value ? null : v.value.id;
  emit("spill", spill.vca);
}
</script>

<template>
  <section class="strip vca" :class="{ muted: v.mute }" :style="{ '--colour': v.colour || 'var(--border)' }" :data-vca="v.id">
    <button class="sname" :aria-pressed="spilled" :title="`${v.name} (${v.id}): ${members.length} members; spill them on the console`" @click="toggleSpill">
      <span class="nm">{{ v.name }}</span>
      <span class="fmt">VCA · {{ members.length }}<span v-if="above.length" :title="`inside ${above.map((x) => x.name).join(', ')}`"> ↑{{ above.length }}</span><span v-if="v.recall_safe" class="safe" title="recall-safe"> S</span></span>
    </button>
    <button class="btn small spill" :class="spilled ? '' : 'secondary'" :aria-pressed="spilled" :aria-label="`Spill ${v.name}`" @click="toggleSpill">Spill</button>
    <div class="upper vcamembers muted small">
      <span v-for="m in members.slice(0, 8)" :key="m">{{ m }}</span>
      <span v-if="members.length > 8">+{{ members.length - 8 }}</span>
      <span v-if="!members.length">no members</span>
    </div>
    <div class="solo">
      <button class="btn small" :class="soloed[v.id] === 'pfl' ? 'pfl' : 'secondary'" :aria-pressed="soloed[v.id] === 'pfl'" :aria-label="`PFL ${v.name}`" title="PFL of every member" @click="toggleSolo(v.id, 'pfl')">PFL</button>
      <button v-if="!compact" class="btn small" :class="soloed[v.id] === 'afl' ? 'pfl' : 'secondary'" :aria-pressed="soloed[v.id] === 'afl'" :aria-label="`AFL ${v.name}`" title="AFL of every member" @click="toggleSolo(v.id, 'afl')">AFL</button>
    </div>
    <button class="btn mute" :class="v.mute ? 'danger' : 'secondary'" :aria-pressed="v.mute" :aria-label="`Mute ${v.name}`" @click="patchVca(v.id, { mute: !v.mute })">Mute</button>
    <div class="faderrow">
      <VFader :value="v.fader_db" :label="`${v.name} VCA fader`" :colour="v.colour || ''" @change="(x) => patchVca(v.id, { fader_db: x })" />
    </div>
  </section>
</template>
