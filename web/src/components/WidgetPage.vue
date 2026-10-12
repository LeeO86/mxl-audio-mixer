<script setup>
// Operator-screen widgets (§12.6, Appendix B.2): /widget/strip?strip=<id>,
// /widget/fader-bank?layout=<name>&page=<name or 1-based number> or ?strips=<id,id,…>,
// /widget/loudness?signal=<id>, each with [&theme=dark|light|transparent], without the app around them,
// on this mixer's own API (same origin). The page posts {type: "widget-ready"} and
// {type: "widget-size", w, h} to the page that frames it.
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import Pill from "./Pill.vue";
import Strip from "./Strip.vue";
import { api } from "../api.js";
import { act, isBus, live, signalOf, startLive, stopLive, stored, wantMeters } from "../store.js";

const query = location.search;
const params = new URLSearchParams(query);
const id = location.pathname.replace(/\/$/, "").split("/").pop();
const theme = params.get("theme");
if (theme) document.documentElement.dataset.theme = theme;
// A frame is see-through only when its color scheme matches its parent's; without the meta it is the default.
if (theme === "transparent") document.querySelector('meta[name="color-scheme"]')?.remove();
const scale = stored("meter.scale", "dbfs");
const alignment = Number(stored("meter.alignment", -18));

const shown = computed(() => {
  if (id === "strip") return [signalOf(params.get("strip"))].filter(Boolean);
  if (id === "loudness") return [signalOf(params.get("signal"))].filter(Boolean);
  if (params.get("strips")) {
    return params
      .get("strips")
      .split(",")
      .map((s) => signalOf(s.trim()))
      .filter(Boolean);
  }
  const layout = (live.mix?.layouts || []).find((l) => l.name === params.get("layout"));
  const page = params.get("page") || "";
  const n = /^\d+$/.test(page) ? Number(page) : 0;
  const p = n >= 1 ? layout?.pages[n - 1] : layout?.pages.find((x) => x.name === page);
  return (p?.signals || []).map(signalOf).filter(Boolean);
});
const ids = computed(() => shown.value.map((s) => s.id));
watch(ids, (list) => wantMeters(list.length ? list : null), { immediate: true });

const signal = computed(() => shown.value[0] || null);
const loud = computed(() => (signal.value ? live.loudness[signal.value.id] : null));
const settings = computed(() => live.mix?.settings || {});
const lufs = (v) => (v === null || v === undefined || v === "-inf" || !Number.isFinite(Number(v)) || Number(v) < -199 ? "−∞" : Number(v).toFixed(1));
const loudAct = (action) => act(() => api.post(`/api/v1/loudness/${signal.value.id}/${action}`));
const measure = () => act(() => api.patch(`/api/v1/${isBus(signal.value.id) ? "buses" : "strips"}/${signal.value.id}`, { loudness: true }));

const root = ref(null);
let ready = false;
let observer = null;
function post(message) {
  if (window.parent !== window) window.parent.postMessage(message, "*");
}
function postSize() {
  const rect = root.value.getBoundingClientRect();
  post({ type: "widget-size", w: Math.round(rect.width), h: Math.round(rect.height) });
}
watch(
  () => shown.value.length,
  (n) => {
    if (!n || ready) return;
    ready = true;
    post({ type: "widget-ready" });
    postSize();
  },
  { flush: "post" },
);
onMounted(() => {
  startLive();
  observer = new ResizeObserver(() => ready && postSize());
  observer.observe(root.value);
});
onUnmounted(() => {
  observer?.disconnect();
  stopLive();
});
</script>

<template>
  <div ref="root" class="widget-root" :class="`widget-${id}`">
    <template v-if="shown.length && id !== 'loudness'">
      <div class="wstrips">
        <Strip v-for="s in shown" :key="s.id" :signal="s" :bus="isBus(s.id)" :scale="scale" :alignment="alignment" compact />
      </div>
    </template>
    <template v-else-if="signal && id === 'loudness'">
      <div class="widget-head">
        <span class="name">{{ signal.name }}</span>
        <span class="muted">{{ settings.loudness_target_lufs }} LUFS ± {{ settings.loudness_tolerance_lu }}</span>
        <span class="spacer"></span>
        <Pill v-if="loud" :text="loud.running ? (loud.in_tolerance ? 'in tolerance' : 'off target') : 'stopped'" :kind="loud.running ? (loud.in_tolerance ? 'ok' : 'warn') : 'neutral'" />
      </div>
      <template v-if="loud">
        <div class="wloud">
          <div><span class="cap">M</span><span class="num">{{ lufs(loud.momentary_lufs) }}</span></div>
          <div><span class="cap">S</span><span class="num">{{ lufs(loud.short_term_lufs) }}</span></div>
          <div class="big" :class="loud.in_tolerance ? 'ok' : 'warn'"><span class="cap">I</span><span class="num">{{ lufs(loud.integrated_lufs) }}</span></div>
          <div><span class="cap">LRA</span><span class="num">{{ lufs(loud.range_lu) }}</span></div>
          <div><span class="cap">TP max</span><span class="num">{{ lufs(loud.max_true_peak_dbtp) }}</span></div>
        </div>
        <div class="btn-grid two">
          <button v-if="loud.running" class="btn secondary" @click="loudAct('stop')">Stop</button>
          <button v-else class="btn" @click="loudAct('start')">Start</button>
          <button class="btn secondary" @click="loudAct('reset')">Reset</button>
        </div>
      </template>
      <div v-else class="empty">
        Not measured.
        <button class="btn small" @click="measure">Measure</button>
      </div>
    </template>
    <div v-else class="empty">{{ live.error || (live.mix ? `Nothing to show for ${query}` : "Connecting…") }}</div>
    <div v-if="live.actionError" class="widget-error" title="Dismiss" @click="live.actionError = ''">{{ live.actionError }}</div>
  </div>
</template>
