<script setup>
// The mixer UI (§12): header with state pills, banners, tabs addressed by the URL hash, and the theme
// (follows the system, or light or dark; remembered in this browser). The browser tab's title names the
// tab shown and the node ("Console – <label> – MXL Audio Mixer"), so many open tabs can be told apart.
import { computed, onMounted, onUnmounted, ref, watch, watchEffect } from "vue";
import Pill from "./components/Pill.vue";
import ConsoleTab from "./components/ConsoleTab.vue";
import ControlRoomTab from "./components/ControlRoomTab.vue";
import ChainTab from "./components/ChainTab.vue";
import RoutingTab from "./components/RoutingTab.vue";
import GraphTab from "./components/GraphTab.vue";
import LogicTab from "./components/LogicTab.vue";
import SnapshotsTab from "./components/SnapshotsTab.vue";
import MetersTab from "./components/MetersTab.vue";
import NmosTab from "./components/NmosTab.vue";
import StatusTab from "./components/StatusTab.vue";
import SettingsTab from "./components/SettingsTab.vue";
import { buses, live, settingValue, startLive, stopLive, store, stored, strips } from "./store.js";
import { midi, startMidi } from "./midi.js";

const tabs = [
  { id: "console", label: "Console", component: ConsoleTab, full: true },
  { id: "control-room", label: "Control room", component: ControlRoomTab, full: true },
  { id: "processing", label: "Processing", component: ChainTab, full: true },
  { id: "routing", label: "Routing", component: RoutingTab, full: true },
  { id: "graph", label: "Graph", component: GraphTab, full: true },
  { id: "logic", label: "Logic", component: LogicTab },
  { id: "snapshots", label: "Snapshots", component: SnapshotsTab },
  { id: "meters", label: "Meters", component: MetersTab, full: true },
  { id: "nmos", label: "NMOS", component: NmosTab },
  { id: "status", label: "Status", component: StatusTab },
  { id: "settings", label: "Settings", component: SettingsTab },
];

const current = ref("console");
const tab = computed(() => tabs.find((t) => t.id === current.value));

watchEffect(() => {
  const label = live.status?.label;
  document.title = [tab.value.label, label, "MXL Audio Mixer"].filter(Boolean).join(" – ");
});

function onHash() {
  const id = location.hash.slice(1);
  if (tabs.some((t) => t.id === id)) current.value = id;
}
function go(id) {
  current.value = id;
  location.hash = id;
}

const theme = ref(stored("theme", "system"));
const THEMES = { system: "Theme: system", light: "Theme: light", dark: "Theme: dark" };
function applyTheme() {
  if (theme.value === "system") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", theme.value);
}
function cycleTheme() {
  theme.value = { system: "light", light: "dark", dark: "system" }[theme.value];
}
watch(theme, (t) => (store("theme", t), applyTheme()));

const ready = computed(() => live.status?.readiness);
const registration = computed(() => {
  const n = live.status?.nmos;
  if (!n) return null;
  if (!n.registry) return { text: "no registry", kind: "neutral" };
  return n.registered ? { text: "registered", kind: "ok" } : { text: "not registered", kind: "warn" };
});
const load = computed(() => live.status?.dsp?.load_ratio ?? null);
const missingInputs = computed(() => (live.status?.inputs || []).filter((i) => i.state !== "running" && i.state !== "unrouted").length);
const outputErrors = computed(() => (live.status?.outputs || []).reduce((n, o) => n + (o.errors || 0), 0));
const masterLost = computed(() => live.status?.link?.role === "slave" && live.status?.link?.seen && !live.status?.link?.up);
const takeoverDenied = computed(() => masterLost.value && settingValue("MIXER_LINK_TAKEOVER") === "deny");

onMounted(() => {
  applyTheme();
  onHash();
  window.addEventListener("hashchange", onHash);
  startLive();
  // MIDI control surfaces (A7) come back on their own once connected in this browser.
  if (midi.enabled) startMidi();
});
onUnmounted(() => {
  window.removeEventListener("hashchange", onHash);
  stopLive();
});
</script>

<template>
  <header>
    <h1>mxl-audio-mixer</h1>
    <span v-if="live.status" class="node">{{ live.status.label }} · {{ strips.length }} strips · {{ buses.length }} buses</span>
    <span class="spacer"></span>
    <Pill v-if="ready" :text="ready.engine_running ? 'running' : 'stopped'" :kind="ready.engine_running ? 'ok' : 'bad'" title="the DSP thread" />
    <Pill v-if="ready && !ready.timing_trusted" text="timing untrusted" kind="bad" title="the kernel TAI offset is not set (docs/development.md)" />
    <Pill v-if="load !== null" :text="`DSP ${(load * 100).toFixed(0)} %`" :kind="load > 0.7 ? 'bad' : load > 0.5 ? 'warn' : 'ok'" title="processing time of the last block / block duration" />
    <Pill v-if="missingInputs" :text="`${missingInputs} input${missingInputs === 1 ? '' : 's'} waiting`" kind="warn" title="routed inputs that are not running" />
    <Pill v-if="outputErrors" :text="`${outputErrors} output errors`" kind="bad" title="MXL commit errors since start" />
    <Pill v-if="registration" :text="registration.text" :kind="registration.kind" title="NMOS registration" />
    <Pill :text="live.connected ? 'live' : 'offline'" :kind="live.connected ? 'ok' : 'bad'" title="/api/v1/events" />
    <button class="btn small secondary" :title="THEMES[theme]" :aria-label="THEMES[theme]" @click="cycleTheme">{{ { system: "◐", light: "☀", dark: "☾" }[theme] }}</button>
    <span v-if="live.status" class="muted small">v{{ live.status.version }} · MXL {{ (live.status.mxl_revision || "").slice(0, 7) }}</span>
  </header>
  <div v-if="live.error" class="banner bad">{{ live.error }}</div>
  <div v-else-if="live.everConnected && !live.connected" class="banner warn">Live updates lost. Reconnecting; values refresh every 2 s meanwhile.</div>
  <div v-if="ready && !ready.timing_trusted" class="banner bad">Timing is not trusted: the kernel TAI offset is not set. Blocks may be scheduled on the wrong clock.</div>
  <div v-if="masterLost" class="banner warn">Master lost. The far-side mix is held.<template v-if="takeoverDenied"> Local changes are refused until the master is back.</template></div>
  <div v-if="live.actionError" class="banner bad">
    {{ live.actionError }}
    <span class="spacer"></span>
    <button class="btn small secondary" @click="live.actionError = ''">Dismiss</button>
  </div>
  <div v-if="live.notice" class="banner info">
    {{ live.notice }}
    <span class="spacer"></span>
    <button class="btn small secondary" @click="live.notice = ''">Dismiss</button>
  </div>
  <nav>
    <button v-for="t in tabs" :key="t.id" :class="{ active: current === t.id }" :aria-current="current === t.id ? 'page' : undefined" @click="go(t.id)">
      {{ t.label }}
    </button>
  </nav>
  <main :class="{ full: tab.full }">
    <!-- The graph keeps its zoom, pan and panels across a tab switch (C5). -->
    <KeepAlive include="GraphTab">
      <component :is="tab.component" @open="(to) => go(to?.tab || 'processing')" />
    </KeepAlive>
  </main>
</template>
