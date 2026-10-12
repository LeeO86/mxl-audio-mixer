<script setup>
// A channel fader (§12.1): drag the cap (mouse, touch, pen), − / + for 0.5 dB steps (held: repeated),
// arrow keys (shift: 0.1 dB), Home for 0 dB, End for −∞, double-click for 0 dB. While dragged, it shows
// its own value so pushes from the server do not make it jump.
import { computed, onUnmounted, ref, watch } from "vue";
import { FADER_MARKS, dbToPos, posToDb } from "../dsp.js";
import { dbValue, fmtDb } from "../api.js";

const props = defineProps({
  value: { type: [Number, String], default: 0 }, // dB or "-inf"
  label: { type: String, default: "Fader" },
  colour: { type: String, default: "" },
});
const emit = defineEmits(["change"]);

const track = ref(null);
const drag = ref(null);
// The last value sent, until the server's value catches up (or 500 ms): quick steps add up.
const pending = ref(null);
let pendingTimer = 0;
watch(
  () => props.value,
  (v) => {
    if (pending.value !== null && dbValue(v) === pending.value) pending.value = null;
  },
);
const db = computed(() => drag.value ?? pending.value ?? dbValue(props.value));
const pos = computed(() => dbToPos(db.value));
const toDb = (d) => (Number.isFinite(d) ? Math.round(d * 10) / 10 : -Infinity);
const out = (d) => (Number.isFinite(d) ? Math.round(d * 10) / 10 : "-inf");
function send(d) {
  pending.value = toDb(d);
  clearTimeout(pendingTimer);
  pendingTimer = setTimeout(() => (pending.value = null), 500);
  emit("change", out(d));
}

let startY = 0;
let startPos = 0;
function down(ev) {
  if (ev.button !== 0) return;
  ev.currentTarget.setPointerCapture?.(ev.pointerId);
  const rect = track.value.getBoundingClientRect();
  startY = ev.clientY;
  // A press on the cap grabs it; a press elsewhere on the track jumps there.
  if (ev.target.classList.contains("cap")) startPos = pos.value;
  else startPos = 1 - (ev.clientY - rect.top) / rect.height;
  drag.value = posToDb(Math.min(1, Math.max(0, startPos)));
  emit("change", out(drag.value));
}
function move(ev) {
  if (drag.value === null) return;
  const h = track.value.getBoundingClientRect().height;
  const fine = ev.shiftKey ? 0.25 : 1;
  const p = Math.min(1, Math.max(0, startPos - ((ev.clientY - startY) / h) * fine));
  drag.value = posToDb(p);
  emit("change", out(drag.value));
}
function up() {
  if (drag.value !== null) send(drag.value);
  drag.value = null;
}

function stepDb(dir, size = 0.5) {
  const cur = Number.isFinite(db.value) ? db.value : -80;
  const next = Math.min(10, cur + dir * size);
  send(next < -80 ? -Infinity : Math.round(next * 10) / 10);
}
let hold = 0;
function press(dir, ev) {
  if (ev.button !== 0) return;
  ev.currentTarget.setPointerCapture?.(ev.pointerId);
  stepDb(dir);
  hold = setTimeout(function again() {
    stepDb(dir);
    hold = setTimeout(again, 60);
  }, 400);
}
function release() {
  clearTimeout(hold);
}
function key(ev) {
  const size = ev.shiftKey ? 0.1 : 0.5;
  if (ev.key === "ArrowUp" || ev.key === "ArrowRight") stepDb(1, size);
  else if (ev.key === "ArrowDown" || ev.key === "ArrowLeft") stepDb(-1, size);
  else if (ev.key === "Home") send(0);
  else if (ev.key === "End") send(-Infinity);
  else return;
  ev.preventDefault();
}
onUnmounted(() => {
  release();
  clearTimeout(pendingTimer);
});
</script>

<template>
  <div class="vfader">
    <button class="btn secondary step" :aria-label="`${label} up`" @pointerdown="press(1, $event)" @pointerup="release" @pointercancel="release" @contextmenu.prevent>+</button>
    <div
      ref="track"
      class="vtrack"
      role="slider"
      tabindex="0"
      :aria-label="label"
      aria-valuemin="-80"
      aria-valuemax="10"
      :aria-valuenow="Number.isFinite(db) ? db : -80"
      :aria-valuetext="`${fmtDb(db)} dB`"
      @pointerdown="down"
      @pointermove="move"
      @pointerup="up"
      @pointercancel="up"
      @dblclick="send(0)"
      @keydown="key"
    >
      <span v-for="m in FADER_MARKS" :key="m" class="mark" :class="{ zero: m === 0 }" :style="{ bottom: `${dbToPos(m) * 100}%` }">{{ m > 0 ? `+${m}` : m }}</span>
      <span class="groove"></span>
      <span class="cap" :style="{ bottom: `${pos * 100}%`, '--cap': colour || 'var(--muted)' }"></span>
    </div>
    <button class="btn secondary step" :aria-label="`${label} down`" @pointerdown="press(-1, $event)" @pointerup="release" @pointercancel="release" @contextmenu.prevent>−</button>
    <span class="vvalue num">{{ fmtDb(db) }}</span>
  </div>
</template>
