<script setup>
// Gain-sharing auto-mixes (§8.3) in a dialog of their own (C7): per auto-mix its switches, and every strip as
// a tile of the same size (member or not), so adding or removing a member moves nothing. A member's tile
// has its speech, gain, weight and bypass.
import AutoLive from "./AutoLive.vue";
import ConfirmButton from "./ConfirmButton.vue";
import Modal from "./Modal.vue";
import NumberField from "./NumberField.vue";
import { ref } from "vue";
import { api } from "../api.js";
import { act, automixOf, automixes, patchAutomix as patchAm, strips } from "../store.js";

const emit = defineEmits(["close"]);

const amForm = ref({ id: "", name: "" });
async function addAutomix() {
  const body = {};
  if (amForm.value.id.trim()) body.id = amForm.value.id.trim();
  if (amForm.value.name.trim()) body.name = amForm.value.name.trim();
  if (await act(() => api.post("/api/v1/automixes", body), "Auto-mix added.")) amForm.value = { id: "", name: "" };
}
const patchAutomix = (a, body) => patchAm(a.id, body);
const memberOf = (a, id) => a.members.find((m) => m.signal === id) || null;
function setMember(a, id, on) {
  const members = on ? [...a.members, { signal: id, weight_db: 0, bypass: false }] : a.members.filter((m) => m.signal !== id);
  patchAutomix(a, { members });
}
function setMemberField(a, id, field, value) {
  patchAutomix(a, { members: a.members.map((m) => (m.signal === id ? { ...m, [field]: value } : m)) });
}
const otherOf = (a, id) => {
  const other = automixOf(id);
  return other && other.id !== a.id ? other : null;
};
</script>

<template>
  <Modal title="Auto-mixes (gain sharing)" wide @close="emit('close')">
    <div v-for="a in automixes" :key="a.id" class="groupedit amedit" :data-automix-edit="a.id">
      <div class="row tight">
        <input :value="a.name" aria-label="Auto-mix name" style="width: 12rem" @change="patchAutomix(a, { name: $event.target.value })" />
        <label class="check"><input type="checkbox" :checked="a.enabled" @change="patchAutomix(a, { enabled: $event.target.checked })" /> on</label>
        <label class="check" title="while nobody speaks, the gains stay"><input type="checkbox" :checked="a.last_mic_hold" @change="patchAutomix(a, { last_mic_hold: $event.target.checked })" /> last-mic hold</label>
        <label class="check" title="a member without speech counts 10 dB less"><input type="checkbox" :checked="a.vad_weighting" @change="patchAutomix(a, { vad_weighting: $event.target.checked })" /> VAD weighting</label>
        <label class="check" title="a snapshot's recall leaves it"><input type="checkbox" :checked="a.recall_safe" @change="patchAutomix(a, { recall_safe: $event.target.checked })" /> recall-safe</label>
        <span class="spacer"></span>
        <span class="muted small">{{ a.members.length }} member{{ a.members.length === 1 ? "" : "s" }}</span>
        <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/automixes/${a.id}`))" />
      </div>
      <div class="amtiles">
        <div
          v-for="x in strips"
          :key="x.id"
          class="amtile"
          :class="{ member: !!memberOf(a, x.id), other: !!otherOf(a, x.id) }"
          :data-member="memberOf(a, x.id) ? x.id : undefined"
          :data-tile="x.id"
        >
          <label class="check amname" :title="otherOf(a, x.id) ? `in ${otherOf(a, x.id).name}` : ''"
            ><input
              type="checkbox"
              :checked="!!memberOf(a, x.id)"
              :disabled="!!otherOf(a, x.id)"
              :aria-label="`${x.name} in ${a.name}`"
              @change="setMember(a, x.id, $event.target.checked)"
            />
            <span>{{ x.name }}</span></label
          >
          <template v-if="memberOf(a, x.id)">
            <div class="amlive"><AutoLive kind="dot" :id="x.id" /> <AutoLive kind="gain" :id="x.id" /></div>
            <div class="amfields">
              <label>Weight dB <NumberField :min="-30" :max="10" :step="1" :value="memberOf(a, x.id).weight_db" :aria-label="`${x.name} weight`" @change="(v) => setMemberField(a, x.id, 'weight_db', v)" /></label>
              <label class="check"><input type="checkbox" :checked="memberOf(a, x.id).bypass" :aria-label="`${x.name} bypass`" @change="setMemberField(a, x.id, 'bypass', $event.target.checked)" /> bypass</label>
            </div>
          </template>
          <div v-else class="muted small amhint">{{ otherOf(a, x.id) ? `in ${otherOf(a, x.id).name}` : "not a member" }}</div>
        </div>
      </div>
    </div>
    <div v-if="!automixes.length" class="empty">No auto-mix yet.</div>
    <div class="row tight">
      <input v-model="amForm.id" placeholder="id (automatic)" aria-label="New auto-mix id" style="width: 10rem" />
      <input v-model="amForm.name" placeholder="name" aria-label="New auto-mix name" style="width: 12rem" />
      <button class="btn small" @click="addAutomix">+ Auto-mix</button>
    </div>
  </Modal>
</template>
