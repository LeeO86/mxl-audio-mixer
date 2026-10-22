<script setup>
// "My headphones" (§10.1 personal monitoring): this browser's own monitor path, with its own source and PFL,
// heard here by WebRTC, without touching the control room; with "Follow PFL/AFL" it also plays the
// control room's PFL/AFL while it has no solo of its own. Removed by the mixer when nobody listened for
// ten minutes.
import { computed, onMounted, onUnmounted, ref } from "vue";
import Pot from "./Pot.vue";
import { api } from "../api.js";
import { act, buses, room, signalOf, signals } from "../store.js";
import { listening, stopListening, toggleListen } from "../whep.js";

const KEY = "mxl-audio-mixer.personal";
const mine = ref(null);
const error = ref("");
let timer = 0;

async function load() {
  const id = sessionStorage.getItem(KEY);
  if (!id) return;
  try {
    const p = await api.get(`/api/v1/personal/${id}`);
    // Removed meanwhile (Remove pressed while this was on its way): keep it gone.
    if (sessionStorage.getItem(KEY) === id) mine.value = p;
  } catch {
    sessionStorage.removeItem(KEY);
    stopListening(id);
    mine.value = null;
  }
}
onMounted(() => {
  load();
  timer = setInterval(load, 2000);
});
onUnmounted(() => clearInterval(timer));

async function create() {
  const source = buses.value[0] ? { signal: buses.value[0].id } : undefined;
  try {
    const p = await api.post("/api/v1/personal", source ? { source } : {});
    sessionStorage.setItem(KEY, p.id);
    mine.value = p;
    error.value = "";
  } catch (e) {
    error.value = e.message;
  }
}
async function set(body) {
  const p = await act(() => api.patch(`/api/v1/personal/${mine.value.id}`, body));
  if (p && p !== true) mine.value = p;
}
async function solo(id, kind) {
  const on = mine.value.solo[id] === kind;
  const path = `/api/v1/personal/${mine.value.id}/solo/${encodeURIComponent(id)}`;
  const p = await act(() => (on ? api.del(path) : api.put(path, { kind })));
  if (p && p !== true) mine.value = p;
}
async function remove() {
  stopListening(mine.value.id);
  await act(() => api.del(`/api/v1/personal/${mine.value.id}`));
  sessionStorage.removeItem(KEY);
  mine.value = null;
}
const sourceKey = (s) => (s ? ("signal" in s ? `s:${s.signal}` : `i:${s.input}`) : "");
const sourceName = (s) => (!s ? "–" : "signal" in s ? signalOf(s.signal)?.name || s.signal : `Input ${s.input}`);
const buttons = computed(() => (room.value.sources.length ? room.value.sources : buses.value.map((b) => ({ signal: b.id }))));
</script>

<template>
  <div class="panel monitor">
    <h3>
      My headphones <span class="muted" style="text-transform: none; letter-spacing: 0">this browser only</span>
      <span class="spacer"></span>
      <template v-if="mine">
        <button class="btn small" :class="listening[mine.id] ? '' : 'secondary'" :aria-pressed="!!listening[mine.id]" :title="listening[mine.id] || 'listen (WebRTC)'" @click="toggleListen(mine.id)">
          {{ listening[mine.id] === "playing" ? "Listening" : listening[mine.id] ? "Connecting…" : "Listen" }}
        </button>
        <button class="btn small secondary" @click="remove">Remove</button>
      </template>
    </h3>
    <template v-if="mine">
      <div class="srcbuttons" role="group" aria-label="My source">
        <button v-for="s in buttons" :key="sourceKey(s)" class="btn" :class="sourceKey(s) === sourceKey(mine.source) ? '' : 'secondary'" :aria-pressed="sourceKey(s) === sourceKey(mine.source)" @click="set({ source: s })">
          {{ sourceName(s) }}
        </button>
      </div>
      <Pot label="Level" unit="dB" :value="mine.level_db" :min="-80" :max="12" :def="0" :fine="0.5" @change="(v) => set({ level_db: v })" />
      <div class="mbuttons">
        <button class="btn secondary on-warn" :aria-pressed="mine.dim" @click="set({ dim: !mine.dim })">Dim</button>
        <button class="btn secondary on-bad" :aria-pressed="mine.cut" @click="set({ cut: !mine.cut })">Mute</button>
      </div>
      <label class="check" title="while the control room has a PFL or AFL and I have none of my own, I hear it">
        <input type="checkbox" :checked="mine.follows_solo" aria-label="Follow PFL/AFL" @change="set({ follows_solo: $event.target.checked })" /> Follow PFL/AFL of the control room
      </label>
      <div class="group-caption">My PFL (the control room is not touched; it wins over the control room's)</div>
      <div class="soloed">
        <button v-for="s in signals" :key="s.id" class="btn small" :class="mine.solo[s.id] === 'pfl' ? 'pfl' : 'secondary'" :aria-pressed="mine.solo[s.id] === 'pfl'" :aria-label="`My PFL ${s.name}`" @click="solo(s.id, 'pfl')">
          {{ s.name }}
        </button>
      </div>
    </template>
    <template v-else>
      <p class="note" style="margin-top: 0">Your own monitor path, heard in this browser, with its own source and PFL.</p>
      <button class="btn" @click="create">Create my headphones</button>
      <p v-if="error" class="msg err">{{ error }}</p>
    </template>
  </div>
</template>
