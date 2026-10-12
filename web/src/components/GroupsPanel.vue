<script setup>
// VCAs and mute groups (§9): their members, colour, recall-safe; add and remove them. A strip or bus is
// mute-safe (isolated from mute groups) in its settings.
import { ref } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import { api } from "../api.js";
import { act, buses, muteGroups, patchMuteGroup, patchVca, strips, vcas } from "../store.js";

const emit = defineEmits(["close"]);
const vcaForm = ref({ id: "", name: "" });
const groupForm = ref({ id: "", name: "" });
async function addVca() {
  const body = {};
  if (vcaForm.value.id.trim()) body.id = vcaForm.value.id.trim();
  if (vcaForm.value.name.trim()) body.name = vcaForm.value.name.trim();
  if (await act(() => api.post("/api/v1/vcas", body), "VCA added.")) vcaForm.value = { id: "", name: "" };
}
async function addGroup() {
  const body = {};
  if (groupForm.value.id.trim()) body.id = groupForm.value.id.trim();
  if (groupForm.value.name.trim()) body.name = groupForm.value.name.trim();
  if (await act(() => api.post("/api/v1/mute-groups", body), "Mute group added.")) groupForm.value = { id: "", name: "" };
}
const toggle = (list, id, on) => (on ? [...list, id] : list.filter((x) => x !== id));
const setVcaMember = (v, id, on) => act(() => api.patch(`/api/v1/vcas/${v.id}`, { members: toggle(v.members, id, on) }));
const setGroupMember = (g, id, on) => act(() => api.patch(`/api/v1/mute-groups/${g.id}`, { members: toggle(g.members, id, on) }));
</script>

<template>
  <div class="panel groups">
    <h3>VCAs and mute groups <span class="spacer"></span><button class="btn small secondary" @click="emit('close')">Close</button></h3>
    <div class="grid two">
      <div>
        <div class="caption">VCAs</div>
        <div v-for="v in vcas" :key="v.id" class="groupedit" :data-vca-edit="v.id">
          <div class="row">
            <input :value="v.name" aria-label="VCA name" @change="patchVca(v.id, { name: $event.target.value })" />
            <input type="color" :value="v.colour || '#5c6773'" aria-label="VCA colour" @change="patchVca(v.id, { colour: $event.target.value })" />
            <label class="check"><input type="checkbox" :checked="v.recall_safe" @change="patchVca(v.id, { recall_safe: $event.target.checked })" /> recall-safe</label>
            <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/vcas/${v.id}`))" />
          </div>
          <div class="members">
            <label v-for="x in [...strips, ...buses]" :key="x.id" class="check"><input type="checkbox" :checked="v.members.includes(x.id)" :aria-label="`${x.name} in ${v.name}`" @change="setVcaMember(v, x.id, $event.target.checked)" /> {{ x.name }}</label>
            <label v-for="x in vcas.filter((o) => o.id !== v.id)" :key="x.id" class="check" title="nested VCA"><input type="checkbox" :checked="v.members.includes(x.id)" :aria-label="`${x.name} in ${v.name}`" @change="setVcaMember(v, x.id, $event.target.checked)" /> VCA {{ x.name }}</label>
          </div>
        </div>
        <div class="row">
          <input v-model="vcaForm.id" placeholder="id (automatic)" aria-label="New VCA id" />
          <input v-model="vcaForm.name" placeholder="name" aria-label="New VCA name" />
          <button class="btn small" @click="addVca">+ VCA</button>
        </div>
      </div>
      <div>
        <div class="caption">Mute groups</div>
        <div v-for="g in muteGroups" :key="g.id" class="groupedit" :data-group-edit="g.id">
          <div class="row">
            <input :value="g.name" aria-label="Mute group name" @change="patchMuteGroup(g.id, { name: $event.target.value })" />
            <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/mute-groups/${g.id}`))" />
          </div>
          <div class="members">
            <label v-for="x in [...strips, ...buses]" :key="x.id" class="check"><input type="checkbox" :checked="g.members.includes(x.id)" :aria-label="`${x.name} in ${g.name}`" @change="setGroupMember(g, x.id, $event.target.checked)" /> {{ x.name }}{{ x.mute_safe ? " (safe)" : "" }}</label>
          </div>
        </div>
        <div class="row">
          <input v-model="groupForm.id" placeholder="id (automatic)" aria-label="New mute group id" />
          <input v-model="groupForm.name" placeholder="name" aria-label="New mute group name" />
          <button class="btn small" @click="addGroup">+ Mute group</button>
        </div>
      </div>
    </div>
  </div>
</template>
