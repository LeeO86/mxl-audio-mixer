<script setup>
// Builds the gallery mix-minus and the far-side in-ears (§13). Choices stay
// while the desk refreshes.
import { computed, ref, watch } from "vue";
import { api } from "../api.js";
import { act, buses, strips } from "../store.js";

const props = defineProps({
  peer: { type: Object, required: true },
});

const ofBus = ref("");
const without = ref([]);
const returnStrip = ref("");
const voices = ref([]);
const ready = ref(false);

watch(
  () => props.peer.desk,
  (desk) => {
    if (!desk || ready.value) return;
    ofBus.value = buses.value[0]?.id || "";
    const named = desk.strips.find((s) => /return|gallery|feed/i.test(`${s.id} ${s.name}`));
    returnStrip.value = named?.id || "";
    voices.value = desk.strips.map((s) => s.id).filter((id) => id !== returnStrip.value);
    ready.value = true;
  },
  { immediate: true },
);
watch(returnStrip, (id) => {
  voices.value = voices.value.filter((v) => v !== id);
});
watch(buses, (list) => {
  if (!ofBus.value && list.length) ofBus.value = list[0].id;
});

const can = computed(() => ofBus.value && returnStrip.value && voices.value.length);
function toggle(which, id) {
  const list = which === "without" ? without : voices;
  const has = list.value.includes(id);
  list.value = has ? list.value.filter((x) => x !== id) : [...list.value, id];
}
async function build() {
  await act(
    () =>
      api.post(`/api/v1/remotes/${encodeURIComponent(props.peer.id)}/return`, {
        of: ofBus.value,
        without: without.value,
        return_strip: returnStrip.value,
        voices: voices.value,
      }),
    "Gallery return published. The far-side in-ears add it to the local voices.",
  );
}
</script>

<template>
  <div class="panel returnform">
    <p class="note">
      The far side mixes its own voices into the in-ears, so that mix does not cross the link and come back. This builds the gallery mix-minus — the programme without the strips that are the far side on this desk — publishes it, and adds that return to each in-ear. Route the published output to the far-side strip chosen as the return.
    </p>
    <div class="row">
      <div>
        <label>Programme</label>
        <select v-model="ofBus" aria-label="Programme bus for the return">
          <option v-for="b in buses" :key="b.id" :value="b.id">{{ b.name }}</option>
        </select>
      </div>
      <div>
        <label>Far-side strip that receives the return</label>
        <select v-model="returnStrip" aria-label="Far-side return strip">
          <option value="">choose…</option>
          <option v-for="s in peer.desk.strips" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
      </div>
    </div>
    <div class="row checks">
      <fieldset>
        <legend>On this desk, these strips are the far side (left out of the return)</legend>
        <label v-for="s in strips" :key="s.id"><input type="checkbox" :checked="without.includes(s.id)" @change="toggle('without', s.id)" /> {{ s.name }}</label>
        <span v-if="!strips.length" class="muted">No local strips yet.</span>
      </fieldset>
      <fieldset>
        <legend>Far-side voices (each in-ear is the others, plus the return)</legend>
        <label v-for="s in peer.desk.strips" :key="s.id" :class="{ muted: s.id === returnStrip }">
          <input type="checkbox" :checked="voices.includes(s.id)" :disabled="s.id === returnStrip" @change="toggle('voices', s.id)" /> {{ s.name }}
        </label>
      </fieldset>
    </div>
    <div class="actions">
      <button class="btn" :disabled="!can" @click="build">Build return and in-ears</button>
    </div>
  </div>
</template>
