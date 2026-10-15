<script setup>
// The far side, drawn on the gallery console (§13). The operator rides those
// strips and auxiliaries as local channels. The voices are mixed on the far side.
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import Pill from "./Pill.vue";
import RemoteReturn from "./RemoteReturn.vue";
import RemoteStrip from "./RemoteStrip.vue";
import { api } from "../api.js";
import { act, live } from "../store.js";

defineProps({
  scale: { type: String, default: "dbfs" },
  alignment: { type: Number, default: -18 },
});

const peers = ref([]);
let timer = 0;

const role = computed(() => live.status?.link?.role || "");
async function load() {
  if (role.value !== "master") return;
  try {
    peers.value = (await api.get("/api/v1/remotes")).peers || [];
  } catch {
    /* the banner tells */
  }
}
watch(role, load);
onMounted(() => {
  load();
  timer = setInterval(load, 250);
});
onUnmounted(() => clearInterval(timer));

function command(peer, body) {
  return act(() => api.post(`/api/v1/remotes/${encodeURIComponent(peer)}/command`, body));
}
function namesOf(desk) {
  const names = {};
  for (const s of [...(desk?.strips || []), ...(desk?.buses || [])]) names[s.id] = s.name;
  return names;
}
function sendsFrom(desk, id) {
  return (desk?.sends || []).filter((s) => s.from === id);
}
function peakOf(peer, id) {
  const v = peer.peaks?.[id];
  return typeof v === "number" ? v : null;
}
</script>

<template>
  <section v-if="role === 'master'" class="remotedesk">
    <div class="toolbar">
      <span class="caption">Far side</span>
      <span v-if="!peers.length" class="muted">Waiting for the far-side mixer.</span>
    </div>
    <article v-for="p in peers" :key="p.id" class="remotepeer" :data-remote="p.id">
      <div class="toolbar">
        <strong>{{ p.label || p.id }}</strong>
        <Pill :text="p.up ? 'linked' : 'link down'" :kind="p.up ? 'ok' : 'bad'" />
        <span v-if="p.rtt_ms != null" class="muted small">{{ Math.round(p.rtt_ms) }} ms</span>
        <span v-if="p.return_bus" class="muted small">return <code>{{ p.return_bus }}</code> published</span>
        <span v-if="p.last_error" class="muted small">{{ p.last_error }}</span>
      </div>
      <div v-if="!p.desk" class="empty">Linked. Waiting for the desk.</div>
      <div v-else class="console">
        <div class="strips">
          <RemoteStrip
            v-for="s in p.desk.strips"
            :key="s.id"
            :signal="s"
            :sends="sendsFrom(p.desk, s.id)"
            :names="namesOf(p.desk)"
            :peak="peakOf(p, s.id)"
            :scale="scale"
            :alignment="alignment"
            @fader="(db) => command(p.id, { op: 'fader', signal: s.id, db })"
            @mute="(mute) => command(p.id, { op: 'mute', signal: s.id, mute })"
            @send="(x, db, mute) => command(p.id, { op: 'send', from: x.from, to: x.to, db, mute: mute ?? x.mute })"
          />
        </div>
        <div v-if="p.desk.buses.length" class="busesec">
          <RemoteStrip
            v-for="b in p.desk.buses"
            :key="b.id"
            :signal="b"
            bus
            :sends="sendsFrom(p.desk, b.id)"
            :names="namesOf(p.desk)"
            :peak="peakOf(p, b.id)"
            :scale="scale"
            :alignment="alignment"
            @fader="(db) => command(p.id, { op: 'fader', signal: b.id, db })"
            @mute="(mute) => command(p.id, { op: 'mute', signal: b.id, mute })"
            @send="(x, db, mute) => command(p.id, { op: 'send', from: x.from, to: x.to, db, mute: mute ?? x.mute })"
          />
        </div>
      </div>
      <RemoteReturn v-if="p.desk" :peer="p" />
    </article>
  </section>
</template>
