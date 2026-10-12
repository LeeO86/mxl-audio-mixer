<script setup>
// Snapshots, layouts and the show history (§12.1, §12.5). A recall can be limited to signals, sections
// (strips, buses) and parameter classes; recall-safe signals are never touched; faders and every other
// value move over the crossfade time.
import { computed, onMounted, ref, watch } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import Pot from "./Pot.vue";
import { api, download } from "../api.js";
import { act, live, patchSignal, signals, store, stored, vcas } from "../store.js";

const snapshots = computed(() => live.mix?.snapshots || []);
const name = ref("");
const fmtTime = (s) => new Date(s * 1000).toLocaleString();
const fmtTimeMs = (ms) => new Date(ms).toLocaleString();

async function take() {
  const body = name.value.trim() ? { name: name.value.trim() } : {};
  if (await act(() => api.post("/api/v1/snapshots", body), "Snapshot taken.")) name.value = "";
}

const CLASSES = [
  ["trim", "Trim and polarity"],
  ["fader", "Faders"],
  ["mute", "Mutes"],
  ["pan", "Pan and width"],
  ["chain", "Processing"],
  ["sends", "Sends"],
];
const classes = ref(JSON.parse(stored("recall.classes", JSON.stringify(CLASSES.map((c) => c[0])))));
const sections = ref(JSON.parse(stored("recall.sections", '["strips","buses","vcas"]')));
const only = ref([]); // empty: every signal
const crossfade = ref(Number(stored("recall.crossfade", 0)));
watch(classes, (v) => store("recall.classes", JSON.stringify(v)), { deep: true });
watch(sections, (v) => store("recall.sections", JSON.stringify(v)), { deep: true });
watch(crossfade, (v) => store("recall.crossfade", v));

function recall(s) {
  const body = { crossfade_ms: crossfade.value, classes: classes.value, sections: sections.value };
  if (only.value.length) body.signals = only.value;
  act(() => api.post(`/api/v1/snapshots/${s.id}/recall`, body), `"${s.name}" recalled.`);
}
const retake = (s) => act(() => api.put(`/api/v1/snapshots/${s.id}`), `"${s.name}" updated with the current mix.`);
const remove = (s) => act(() => api.del(`/api/v1/snapshots/${s.id}`));
const renaming = ref("");
const newName = ref("");
async function rename(s) {
  if (newName.value.trim() && (await act(() => api.patch(`/api/v1/snapshots/${s.id}`, { name: newName.value.trim() })))) renaming.value = "";
}

// Layouts: edited locally, saved as a whole.
const draft = ref([]);
const dirty = ref(false);
watch(
  () => live.mix?.layouts,
  (l) => {
    if (!dirty.value) draft.value = JSON.parse(JSON.stringify(l || []));
  },
  { immediate: true },
);
const edit = (fn) => {
  fn();
  dirty.value = true;
};
const addLayout = () => edit(() => draft.value.push({ name: `Layout ${draft.value.length + 1}`, pages: [{ name: "Page 1", signals: [] }] }));
const addPage = (l) => edit(() => l.pages.push({ name: `Page ${l.pages.length + 1}`, signals: [] }));
const moveSignal = (p, i, d) =>
  edit(() => {
    const [x] = p.signals.splice(i, 1);
    p.signals.splice(i + d, 0, x);
  });
const nameOf = (id) => [...signals.value, ...vcas.value].find((s) => s.id === id)?.name || id;
async function saveLayouts() {
  if (await act(() => api.put("/api/v1/layouts", draft.value), "Layouts saved.")) dirty.value = false;
}
function revertLayouts() {
  dirty.value = false;
  draft.value = JSON.parse(JSON.stringify(live.mix?.layouts || []));
}

// Show history.
const history = ref([]);
const versionName = ref("");
async function loadHistory() {
  try {
    history.value = await api.get("/api/v1/shows/history");
  } catch {
    /* the banner tells */
  }
}
onMounted(loadHistory);
watch(() => live.revision, loadHistory);
async function keepVersion() {
  if (await act(() => api.post("/api/v1/shows/history", { name: versionName.value.trim() }), "Version kept.")) {
    versionName.value = "";
    loadHistory();
  }
}
const restore = (v) => act(() => api.post(`/api/v1/shows/history/${v.id}/restore`), `Version of ${fmtTimeMs(v.time_ms)} restored; the show before it is kept as a version.`);
async function removeVersion(v) {
  if (await act(() => api.del(`/api/v1/shows/history/${v.id}`))) loadHistory();
}
async function downloadVersion(v) {
  const mix = await act(() => api.get(`/api/v1/shows/history/${v.id}`));
  if (mix) download(`mxl-audio-mixer-show-${v.id}.json`, `${JSON.stringify({ version: 1, mix }, null, 2)}\n`);
}
</script>

<template>
  <div class="grid two">
    <div class="panel">
      <h3>Snapshots</h3>
      <div class="row tight">
        <input v-model="name" placeholder="Name (empty: automatic)" style="flex: 1; width: auto" aria-label="Snapshot name" @keydown.enter="take" />
        <button class="btn" @click="take">Take snapshot</button>
      </div>
      <table style="margin-top: 0.6rem">
        <thead>
          <tr><th>Name</th><th>Taken</th><th class="num">Signals</th><th></th></tr>
        </thead>
        <tbody>
          <tr v-for="s in snapshots" :key="s.id">
            <td>
              <template v-if="renaming === s.id">
                <input v-model="newName" :aria-label="`New name of ${s.name}`" @keydown.enter="rename(s)" @keydown.esc="renaming = ''" />
              </template>
              <template v-else>
                <strong>{{ s.name }}</strong> <span class="muted small">{{ s.id }}</span>
              </template>
            </td>
            <td class="small">{{ fmtTime(s.time) }}</td>
            <td class="num">{{ Object.keys(s.signals).length }}</td>
            <td style="text-align: right; white-space: nowrap">
              <button class="btn small" @click="recall(s)">Recall</button>
              <button v-if="renaming !== s.id" class="btn small secondary" @click="(renaming = s.id), (newName = s.name)">Rename</button>
              <button v-else class="btn small secondary" @click="rename(s)">Save</button>
              <ConfirmButton label="Update" small @confirm="retake(s)" />
              <ConfirmButton label="Delete" small @confirm="remove(s)" />
            </td>
          </tr>
          <tr v-if="!snapshots.length"><td colspan="4" class="empty">No snapshots yet.</td></tr>
        </tbody>
      </table>
    </div>

    <div class="panel">
      <h3>Recall</h3>
      <Pot label="Crossfade" unit="ms" :value="crossfade" :min="0" :max="10000" :def="0" :fine="10" :digits="0" @change="(v) => (crossfade = v)" />
      <div class="group-caption">Sections</div>
      <div class="checks">
        <label class="check"><input v-model="sections" type="checkbox" value="strips" /> Strips</label>
        <label class="check"><input v-model="sections" type="checkbox" value="buses" /> Buses</label>
        <label class="check"><input v-model="sections" type="checkbox" value="vcas" /> VCAs</label>
      </div>
      <div class="group-caption">Parameters</div>
      <div class="checks">
        <label v-for="[v, l] in CLASSES" :key="v" class="check"><input v-model="classes" type="checkbox" :value="v" /> {{ l }}</label>
      </div>
      <div class="group-caption">Signals (none ticked: all) and recall-safe</div>
      <table>
        <tbody>
          <tr v-for="s in signals" :key="s.id">
            <td><label class="check" style="margin: 0"><input v-model="only" type="checkbox" :value="s.id" /> {{ s.name }}</label></td>
            <td style="text-align: right">
              <label class="check" style="margin: 0; justify-content: flex-end" :title="`recalls leave ${s.name} alone`">
                <input type="checkbox" :checked="s.recall_safe" @change="patchSignal(s.id, { recall_safe: $event.target.checked })" /> safe
              </label>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>

  <div class="panel">
    <h3>
      Layouts
      <span class="spacer"></span>
      <button class="btn small secondary" @click="addLayout">+ Layout</button>
      <button class="btn small secondary" :disabled="!dirty" @click="revertLayouts">Revert</button>
      <button class="btn small" :disabled="!dirty" @click="saveLayouts">Save</button>
    </h3>
    <p class="note" style="margin-top: 0">A layout is pages of strips and buses in your order; the console shows one page at a time.</p>
    <div v-for="(l, li) in draft" :key="li" class="panel" style="background: var(--panel2)">
      <div class="row tight">
        <input :value="l.name" aria-label="Layout name" style="width: 16rem" @input="edit(() => (l.name = $event.target.value))" />
        <button class="btn small secondary" @click="addPage(l)">+ Page</button>
        <ConfirmButton label="Delete layout" small @confirm="edit(() => draft.splice(li, 1))" />
      </div>
      <div class="grid fit" style="margin-top: 0.6rem">
        <div v-for="(p, pi) in l.pages" :key="pi" class="bandbox">
          <h4>
            <input :value="p.name" aria-label="Page name" @input="edit(() => (p.name = $event.target.value))" />
            <button class="btn small secondary" :aria-label="`Delete page ${p.name}`" @click="edit(() => l.pages.splice(pi, 1))">✕</button>
          </h4>
          <div v-for="(id, i) in p.signals" :key="id" class="row tight" style="margin: 0.2rem 0">
            <span style="flex: 1">{{ nameOf(id) }}</span>
            <button class="btn small secondary" :disabled="i === 0" @click="moveSignal(p, i, -1)">↑</button>
            <button class="btn small secondary" :disabled="i === p.signals.length - 1" @click="moveSignal(p, i, 1)">↓</button>
            <button class="btn small secondary" @click="edit(() => p.signals.splice(i, 1))">✕</button>
          </div>
          <select :value="''" aria-label="Add a signal" @change="edit(() => p.signals.push($event.target.value)), ($event.target.value = '')">
            <option value="" disabled>+ Add a strip, bus or VCA</option>
            <option v-for="s in signals.filter((x) => !p.signals.includes(x.id))" :key="s.id" :value="s.id">{{ s.name }}</option>
            <option v-for="v in vcas.filter((x) => !p.signals.includes(x.id))" :key="v.id" :value="v.id">VCA {{ v.name }}</option>
          </select>
        </div>
      </div>
    </div>
    <div v-if="!draft.length" class="empty">No layouts: the console shows all strips in banks.</div>
  </div>

  <div class="panel">
    <h3>Show history</h3>
    <p class="note" style="margin-top: 0">A version is kept automatically at most once a minute after a change (the newest 100), and whenever you keep one by name. Restoring keeps the current show as a version first.</p>
    <div class="row tight">
      <input v-model="versionName" placeholder="Name of this version" style="flex: 1; width: auto" aria-label="Version name" @keydown.enter="versionName.trim() && keepVersion()" />
      <button class="btn" :disabled="!versionName.trim()" @click="keepVersion">Keep version</button>
    </div>
    <table style="margin-top: 0.6rem">
      <thead>
        <tr><th>Time</th><th>Name</th><th class="num">Strips</th><th class="num">Buses</th><th class="num">Sends</th><th></th></tr>
      </thead>
      <tbody>
        <tr v-for="v in history" :key="v.id">
          <td class="small">{{ fmtTimeMs(v.time_ms) }}</td>
          <td>{{ v.name || "–" }}</td>
          <td class="num">{{ v.strips }}</td>
          <td class="num">{{ v.buses }}</td>
          <td class="num">{{ v.sends }}</td>
          <td style="text-align: right; white-space: nowrap">
            <button class="btn small secondary" @click="downloadVersion(v)">Download</button>
            <ConfirmButton label="Restore" small @confirm="restore(v)" />
            <ConfirmButton label="Delete" small @confirm="removeVersion(v)" />
          </td>
        </tr>
        <tr v-if="!history.length"><td colspan="6" class="empty">No versions yet.</td></tr>
      </tbody>
    </table>
  </div>
</template>
