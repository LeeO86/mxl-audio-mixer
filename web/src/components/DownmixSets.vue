<script setup>
// Named down-mix settings (C8, 1.6.0): each is a gain matrix for every pair of formats, starting from
// Adrian's downmix.json (the reference, or its "surround to side" variants) and edited cell by cell.
// The mix uses one (or the parametric BS.775 levels of 1.5); a bus or monitor can use another. The
// matrix shows the largest format of the mix (the widest bus or strip) into each narrower one by
// default: rows are the target's channels, columns the source's, cells in dB (empty: off). Edits are a
// draft until Apply; edited cells are marked against the reference.
import { computed, ref, watch } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import DownmixEdit from "./DownmixEdit.vue";
import { DEFAULT_DOWNMIX, FORMATS } from "../nodes.js";
import { api, dbValue } from "../api.js";
import { act, live } from "../store.js";

const LABELS = {
  mono: ["M"],
  stereo: ["L", "R"],
  "3.0": ["L", "R", "C"],
  "5.1": ["L", "R", "C", "LFE", "Ls", "Rs"],
  "7.1": ["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs"],
  "5.1.4": ["L", "R", "C", "LFE", "Ls", "Rs", "Ltf", "Rtf", "Ltr", "Rtr"],
  "7.1.4": ["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs", "Ltf", "Rtf", "Ltr", "Rtr"],
};
const RULES = [
  ["reference", "downmix.json"],
  ["surround-to-side", "downmix.json, surrounds to sides"],
  ["surround-to-side-and-rear", "downmix.json, surrounds to sides and rears"],
];
/** As the server's downmix::applies: the target lacks a channel of the source (not from mono, nor
 * stereo into mono: the pan law and the mono sum do those). */
const applies = (from, to) => from !== "mono" && !(from === "stereo" && to === "mono") && LABELS[from].some((l) => !LABELS[to].includes(l));

const settings = computed(() => live.mix?.settings || {});
const sets = computed(() => settings.value.downmixes || []);
const setMix = (body) => act(() => api.patch("/api/v1/mix/settings", body));
const putSets = (list) => setMix({ downmixes: list });

// The widest format of the mix: what the matrices must cover.
const largest = computed(() => {
  const all = [...(live.mix?.buses || []), ...(live.mix?.strips || [])].map((s) => s.format);
  return all.reduce((a, f) => (LABELS[f].length > LABELS[a].length ? f : a), "stereo");
});

function addSet(rules = "reference") {
  let n = sets.value.length + 1;
  while (sets.value.some((d) => d.id === `downmix-${n}`)) n++;
  const name = RULES.find((r) => r[0] === rules)?.[1] || "Down-mix";
  putSets([...sets.value, { id: `downmix-${n}`, name: `${name} ${n}`, rules, normalise: false }]);
  picked.value = `downmix-${n}`;
}
const change = (id, body) => putSets(sets.value.map((d) => (d.id === id ? { ...d, ...body } : d)));
const remove = (id) => putSets(sets.value.filter((d) => d.id !== id));

// The matrix editor.
const picked = ref("");
watch(
  sets,
  (list) => {
    if (!list.some((d) => d.id === picked.value)) picked.value = settings.value.downmix_set || list[0]?.id || "";
  },
  { immediate: true },
);
const from = ref("");
const to = ref("");
watch(
  largest,
  (f) => {
    from.value = f;
    to.value = FORMATS.find((t) => applies(f, t) && LABELS[t].length < LABELS[f].length && t === "stereo") || FORMATS.find((t) => applies(f, t)) || "";
  },
  { immediate: true },
);
const targets = computed(() => FORMATS.filter((t) => from.value && applies(from.value, t)));
const sources = computed(() => FORMATS.filter((f) => FORMATS.some((t) => applies(f, t))));
const grid = ref(null); // GET /api/v1/downmix-matrix
const draft = ref([]);
const show = (v) => {
  const d = dbValue(v);
  return Number.isFinite(d) ? String(Math.round(d * 100) / 100) : "";
};
async function load() {
  grid.value = null;
  if (!picked.value || !from.value || !to.value || !applies(from.value, to.value)) return;
  const q = new URLSearchParams({ set: picked.value, from: from.value, to: to.value });
  try {
    grid.value = await api.get(`/api/v1/downmix-matrix?${q}`);
    draft.value = grid.value.gains_db.map((row) => row.map(show));
  } catch (e) {
    live.actionError = e.message;
  }
}
watch([picked, from, to, () => JSON.stringify(sets.value)], load, { immediate: true });
const differs = (r, c) => grid.value && show(grid.value.reference_db[r][c]) !== draft.value[r][c];
const dirty = computed(() => grid.value && draft.value.some((row, r) => row.some((v, c) => v !== show(grid.value.gains_db[r][c]))));
function cellValue(v) {
  const t = String(v).trim();
  if (t === "" || t === "-inf" || t === "off") return "-inf";
  const n = Number(t.replace(",", "."));
  return Number.isFinite(n) ? Math.max(-60, Math.min(10, n)) : "-inf";
}
function apply() {
  const set = sets.value.find((d) => d.id === picked.value);
  if (!set) return;
  const gains_db = draft.value.map((row) => row.map(cellValue));
  // The exact reference (to 0.01 dB) is no edit: store only a changed pair.
  const same = gains_db.every((row, r) => row.every((v, c) => show(v) === show(grid.value.reference_db[r][c])));
  const matrices = (set.matrices || []).filter((m) => !(m.from === from.value && m.to === to.value));
  if (!same) matrices.push({ from: from.value, to: to.value, gains_db: gains_db.map((row, r) => row.map((v, c) => (show(v) === show(grid.value.reference_db[r][c]) ? grid.value.reference_db[r][c] : v))) });
  change(set.id, { matrices });
}
function resetPair() {
  const set = sets.value.find((d) => d.id === picked.value);
  if (set) change(set.id, { matrices: (set.matrices || []).filter((m) => !(m.from === from.value && m.to === to.value)) });
}
const editedPairs = computed(() => (sets.value.find((d) => d.id === picked.value)?.matrices || []).map((m) => `${m.from} → ${m.to}`));
</script>

<template>
  <div class="downmixsets">
    <div class="params">
      <div>
        <label>The mix's down-mix</label>
        <select :value="settings.downmix_set || ''" aria-label="Mix down-mix" @change="setMix({ downmix_set: $event.target.value || null })">
          <option value="">Parametric: centre, surround, LFE levels (BS.775, as 1.5)</option>
          <option v-for="d in sets" :key="d.id" :value="d.id">{{ d.name }}</option>
        </select>
        <p class="note">A bus or monitor can use another one (its settings). The matrices cover every format; shown from the widest of this mix: {{ largest }}.</p>
      </div>
      <div v-if="!settings.downmix_set">
        <DownmixEdit :value="settings.downmix || DEFAULT_DOWNMIX" label="Down-mix" @change="(d) => setMix({ downmix: d })" />
      </div>
    </div>

    <table class="sets">
      <thead>
        <tr><th>Setting</th><th>Starts from</th><th>Normalise</th><th>Edited</th><th></th></tr>
      </thead>
      <tbody>
        <tr v-for="d in sets" :key="d.id" :class="{ picked: d.id === picked }" @click="picked = d.id">
          <td><input :value="d.name" :aria-label="`${d.id} name`" @change="change(d.id, { name: $event.target.value.trim() || d.name })" /></td>
          <td>
            <select :value="d.rules || 'reference'" :aria-label="`${d.id} rules`" @change="change(d.id, { rules: $event.target.value })">
              <option v-for="[v, l] in RULES" :key="v" :value="v">{{ l }}</option>
            </select>
          </td>
          <td><input type="checkbox" :checked="d.normalise" :aria-label="`${d.id} normalise`" @change="change(d.id, { normalise: $event.target.checked })" /></td>
          <td class="muted small">{{ (d.matrices || []).length ? (d.matrices || []).map((m) => `${m.from}→${m.to}`).join(", ") : "—" }}</td>
          <td><ConfirmButton small label="Delete" :aria-label="`Delete ${d.name}`" @confirm="remove(d.id)" /></td>
        </tr>
      </tbody>
    </table>
    <div class="row tight" style="margin-top: 0.4rem">
      <button class="btn small secondary" @click="addSet('reference')">+ Down-mix setting</button>
      <button class="btn small secondary" @click="addSet('surround-to-side')">+ Surrounds to sides</button>
    </div>

    <template v-if="picked && sets.length">
      <div class="row tight pairpick" style="margin-top: 0.8rem; align-items: center">
        <label class="muted small">Matrix of</label>
        <select v-model="picked" aria-label="Down-mix setting">
          <option v-for="d in sets" :key="d.id" :value="d.id">{{ d.name }}</option>
        </select>
        <label class="muted small">from</label>
        <select v-model="from" aria-label="Down-mix from">
          <option v-for="f in sources" :key="f" :value="f">{{ f }}</option>
        </select>
        <label class="muted small">into</label>
        <select v-model="to" aria-label="Down-mix into">
          <option v-for="f in targets" :key="f" :value="f">{{ f }}</option>
        </select>
      </div>
      <div v-if="grid" class="matrixwrap">
        <table class="matrix" data-downmix-matrix>
          <thead>
            <tr>
              <th class="muted small">{{ to }} ← {{ from }}</th>
              <th v-for="c in grid.columns" :key="c">{{ c }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(r, ri) in grid.rows" :key="r">
              <th>{{ r }}</th>
              <td v-for="(c, ci) in grid.columns" :key="c" :class="{ edited: differs(ri, ci), on: draft[ri][ci] !== '' }">
                <input v-model="draft[ri][ci]" :aria-label="`${c} into ${r}`" :title="`reference ${show(grid.reference_db[ri][ci]) || 'off'} dB`" placeholder="off" />
              </td>
            </tr>
          </tbody>
        </table>
        <div class="row tight">
          <ConfirmButton small label="Apply" :disabled="!dirty" aria-label="Apply the matrix" @confirm="apply" />
          <button class="btn small secondary" :disabled="!dirty" @click="load">Undo</button>
          <ConfirmButton small label="Reset to the reference" :disabled="!grid.edited" aria-label="Reset the matrix" @confirm="resetPair" />
          <span class="muted small">{{ grid.edited ? "Edited (marked cells differ from downmix.json)." : "As downmix.json." }} Edited pairs: {{ editedPairs.join(", ") || "none" }}.</span>
        </div>
        <p v-if="dirty" class="note">Applying fades the affected sends out and in: a brief dip.</p>
      </div>
    </template>
  </div>
</template>

<style scoped>
.sets {
  margin-top: 0.6rem;
}
.downmixsets .row select {
  width: auto;
}
.pairpick label {
  margin: 0;
}
.sets tr.picked td {
  background: color-mix(in srgb, var(--accent) 12%, transparent);
}
.matrixwrap {
  overflow-x: auto;
  margin-top: 0.4rem;
}
.matrix {
  border-collapse: collapse;
  width: auto;
}
.matrix th,
.matrix td {
  padding: 0.1rem;
  text-align: center;
  font-size: 0.8rem;
}
.matrix input {
  width: 3.4rem;
  padding: 0.1rem 0.2rem;
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.matrix td.on input {
  background: color-mix(in srgb, var(--ok, #4a8) 15%, transparent);
}
.matrix td.edited input {
  outline: 2px solid var(--warn, #c93);
}
</style>
