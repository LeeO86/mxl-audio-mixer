<script setup>
// The high-level operations (§5) on a selection of strips: group them into a subgroup bus, N-1 buses (one per
// participant, or one of the selection), link mono strips into a stereo or surround strip (and unlink a stereo
// one), a mix-minus of a bus. Each makes ordinary buses and sends, editable afterwards.
import { computed, ref } from "vue";
import { api } from "../api.js";
import { act, buses, strips } from "../store.js";

const emit = defineEmits(["close"]);
const selected = ref([]);
const name = ref("");
const n1Of = ref("");
const n1Mode = ref("per-participant");
const n1Format = ref("");
const groupFormat = ref("stereo");
const mmOf = ref("");
const unlinkStrip = ref("");
const result = ref("");
const FORMATS = ["mono", "stereo", "3.0", "5.1", "7.1", "5.1.4", "7.1.4"];
const LINKS = { 2: "stereo", 3: "3.0", 6: "5.1", 8: "7.1", 10: "5.1.4", 12: "7.1.4" };
const monoSelected = computed(() => selected.value.every((id) => strips.value.find((s) => s.id === id)?.format === "mono"));
const linkFormat = computed(() => (monoSelected.value ? LINKS[selected.value.length] : null));
const stereoStrips = computed(() => strips.value.filter((s) => s.format === "stereo"));

async function run(op, body, what) {
  const made = await act(() => api.post(`/api/v1/operations/${op}`, body));
  if (made) {
    const list = [...(made.buses || []), ...(made.strips || [])];
    result.value = `${what}: ${list.join(", ")}${made.removed?.length ? ` (removed ${made.removed.join(", ")})` : ""}`;
    selected.value = selected.value.filter((id) => !(made.removed || []).includes(id));
  }
}
const opt = (v) => (v ? v : undefined);
const group = () => run("group", { strips: selected.value, name: opt(name.value.trim()), format: groupFormat.value }, "Group");
const n1 = () => run("n-1", { strips: selected.value, of: opt(n1Of.value), mode: n1Mode.value, format: opt(n1Format.value) }, "N-1");
const link = () => run("link", { strips: selected.value, name: opt(name.value.trim()) }, "Linked");
const unlink = () => run("unlink", { strip: unlinkStrip.value }, "Unlinked");
const mixMinus = () => run("mix-minus", { of: mmOf.value, without: selected.value, name: opt(name.value.trim()) }, "Mix-minus");
</script>

<template>
  <div class="panel operations">
    <h3>Operations <span class="spacer"></span><button class="btn small secondary" @click="emit('close')">Close</button></h3>
    <div class="caption">Selection</div>
    <div class="members">
      <label v-for="s in strips" :key="s.id" class="check"><input v-model="selected" type="checkbox" :value="s.id" :aria-label="`Select ${s.name}`" /> {{ s.name }} <span class="muted small">{{ s.format }}</span></label>
    </div>
    <div class="row" style="margin-top: 0.4rem">
      <input v-model="name" placeholder="name of what is made (optional)" aria-label="Name of what is made" />
      <span class="muted small">{{ selected.length }} selected</span>
    </div>
    <div class="grid two">
      <div class="opbox">
        <h4>Group</h4>
        <p class="note">A subgroup bus; the selection's post-fader sends move to it, and it sends where they sent.</p>
        <div class="row">
          <select v-model="groupFormat" aria-label="Group format"><option v-for="f in FORMATS" :key="f" :value="f">{{ f }}</option></select>
          <button class="btn small" :disabled="!selected.length" @click="group">Group</button>
        </div>
      </div>
      <div class="opbox">
        <h4>N-1</h4>
        <p class="note">From a bus: it, as it is, minus each participant (or minus the selection). Without: the others of the selection.</p>
        <div class="row">
          <select v-model="n1Of" aria-label="N-1 of">
            <option value="">the selection</option>
            <option v-for="b in buses" :key="b.id" :value="b.id">{{ b.name }}</option>
          </select>
          <select v-model="n1Mode" aria-label="N-1 mode">
            <option value="per-participant">one per participant</option>
            <option value="one" :disabled="!n1Of">one of the selection</option>
          </select>
          <select v-model="n1Format" aria-label="N-1 format"><option value="">format of the bus (or mono)</option><option v-for="f in FORMATS" :key="f" :value="f">{{ f }}</option></select>
          <button class="btn small" :disabled="!selected.length || (n1Mode === 'one' && !n1Of)" @click="n1">Create N-1</button>
        </div>
      </div>
      <div class="opbox">
        <h4>Link</h4>
        <p class="note">Mono strips into one {{ linkFormat || "stereo or surround" }} strip: their patches in order, the first one's processing and fader.</p>
        <div class="row">
          <button class="btn small" :disabled="!linkFormat" @click="link">Link {{ linkFormat || "" }}</button>
          <select v-model="unlinkStrip" aria-label="Stereo strip to unlink"><option value="">stereo strip…</option><option v-for="s in stereoStrips" :key="s.id" :value="s.id">{{ s.name }}</option></select>
          <button class="btn small secondary" :disabled="!unlinkStrip" @click="unlink">Unlink</button>
        </div>
      </div>
      <div class="opbox">
        <h4>Mix-minus</h4>
        <p class="note">A bus that follows another one, without the selection.</p>
        <div class="row">
          <select v-model="mmOf" aria-label="Mix-minus of"><option value="">bus…</option><option v-for="b in buses" :key="b.id" :value="b.id">{{ b.name }}</option></select>
          <button class="btn small" :disabled="!mmOf || !selected.length" @click="mixMinus">Create mix-minus</button>
        </div>
      </div>
    </div>
    <p v-if="result" class="note ok" data-op-result>{{ result }}</p>
  </div>
</template>
