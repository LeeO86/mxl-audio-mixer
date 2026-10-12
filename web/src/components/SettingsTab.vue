<script setup>
// Settings (§8, §12.5): the mix settings (pan law, stereo-to-mono sum, loudness target and tolerance),
// every setting with its value and origin (read at start), and the show file (export, import).
import { computed, onMounted, ref } from "vue";
import Pot from "./Pot.vue";
import Segmented from "./Segmented.vue";
import { api, copyText, download } from "../api.js";
import { act, live, refreshConfig, settingValue } from "../store.js";

const settings = computed(() => live.mix?.settings || {});
const setMix = (body) => act(() => api.patch("/api/v1/mix/settings", body));

const GROUPS = [
  { title: "Mixer", test: /^MIXER_/ },
  { title: "MXL", test: /^MXL_/ },
  { title: "NMOS", test: /^(NMOS_|HOST_ID)/ },
  { title: "Tally and widgets", test: /^(TSL_|WIDGET_)/ },
  { title: "Web and process", test: /./ },
];
const ORIGIN = {
  environment: { text: "ENV", cls: "env" },
  file: { text: "FILE", cls: "file" },
  default: { text: "DEFAULT", cls: "default" },
};
const groups = computed(() => {
  const left = [...(live.config?.settings || [])];
  return GROUPS.map((g) => {
    const items = left.filter((s) => g.test.test(s.key));
    items.forEach((s) => left.splice(left.indexOf(s), 1));
    return { title: g.title, items };
  }).filter((g) => g.items.length);
});

const exportDoc = ref("");
const exportMsg = ref("");
const importText = ref("");
const importMsg = ref({ kind: "", text: "" });
async function loadExport() {
  try {
    exportDoc.value = JSON.stringify(await api.get("/api/v1/config/export"), null, 2);
  } catch (e) {
    exportMsg.value = e.message;
  }
}
onMounted(() => {
  refreshConfig();
  loadExport();
});
async function copy() {
  exportMsg.value = (await copyText(exportDoc.value)) ? "Copied." : "Copy failed; select the text and copy it by hand.";
}
function onFile(ev) {
  ev.target.files?.[0]?.text().then((t) => (importText.value = t));
}
const armed = ref(false);
async function doImport() {
  try {
    JSON.parse(importText.value);
  } catch (e) {
    importMsg.value = { kind: "err", text: `Not JSON: ${e.message}` };
    return;
  }
  if (!armed.value) {
    armed.value = true;
    setTimeout(() => (armed.value = false), 3000);
    return;
  }
  armed.value = false;
  try {
    await api.post("/api/v1/config/import", importText.value);
    importMsg.value = { kind: "ok", text: "Imported: the show (mix, snapshots, layouts) and the input routes are replaced; the current show is in the history." };
    importText.value = "";
    loadExport();
  } catch (e) {
    importMsg.value = { kind: "err", text: e.message };
  }
}
const seed = computed(() => settingValue("NMOS_SEED") || settingValue("HOST_ID") || "mixer");
</script>

<template>
  <div class="panel">
    <h3>Mix</h3>
    <div class="params">
      <div>
        <label>Pan law (centred mono, each side)</label>
        <Segmented :model-value="settings.pan_law_db" :options="[0, -3, -4.5, -6].map((v) => ({ value: v, label: `${v} dB` }))" label="Pan law" @update:model-value="(v) => setMix({ pan_law_db: v })" />
      </div>
      <div>
        <label>Stereo to mono (L + R)</label>
        <Segmented :model-value="settings.mono_sum_db" :options="[-3, -6].map((v) => ({ value: v, label: `${v} dB` }))" label="Stereo to mono" @update:model-value="(v) => setMix({ mono_sum_db: v })" />
      </div>
      <Pot label="Loudness target" unit="LUFS" :value="settings.loudness_target_lufs ?? -23" :min="-60" :max="0" :def="-23" :fine="0.5" @change="(v) => setMix({ loudness_target_lufs: v })" />
      <Pot label="Loudness tolerance" unit="± LU" :value="settings.loudness_tolerance_lu ?? 1" :min="0" :max="10" :def="1" :fine="0.1" @change="(v) => setMix({ loudness_tolerance_lu: v })" />
    </div>
  </div>

  <div class="panel">
    <h3>Configuration</h3>
    <p class="note" style="margin: 0">
      Precedence: environment (ENV), then the configuration file (FILE), then the default. The mixer reads them at start: change them in its deployment and restart it.
      The show is kept in <code>{{ settingValue("MIXER_STATE_DIR") || "MIXER_STATE_DIR" }}</code>.
    </p>
  </div>
  <div v-for="g in groups" :key="g.title" class="panel">
    <h3>{{ g.title }}</h3>
    <table>
      <thead>
        <tr><th style="width: 34%">Key</th><th>Value</th><th style="width: 6rem">Origin</th></tr>
      </thead>
      <tbody>
        <tr v-for="s in g.items" :key="s.key">
          <td>
            <code>{{ s.key }}</code>
            <div class="desc">{{ s.description || "" }}</div>
          </td>
          <td style="overflow-wrap: anywhere">{{ s.value === "" ? "–" : s.value }}</td>
          <td><span class="badge" :class="ORIGIN[s.source]?.cls">{{ ORIGIN[s.source]?.text || s.source }}</span></td>
        </tr>
      </tbody>
    </table>
  </div>

  <div class="grid two">
    <div class="panel">
      <h3>Export show</h3>
      <p class="note">One JSON document: the mix (strips, buses, sends, outputs, processing), snapshots, layouts, the input routes and the settings (for information).</p>
      <div class="actions" style="margin-top: 0">
        <span class="msg ok" style="margin: 0">{{ exportMsg }}</span>
        <span class="spacer"></span>
        <button class="btn secondary" @click="loadExport">Reload</button>
        <button class="btn secondary" :disabled="!exportDoc" @click="copy">Copy</button>
        <button class="btn" :disabled="!exportDoc" @click="download(`mxl-audio-mixer-${seed}.json`, exportDoc + '\n')">Download</button>
      </div>
      <pre style="max-height: 320px">{{ exportDoc }}</pre>
    </div>
    <div class="panel">
      <h3>Import show</h3>
      <p class="note">An exported document. Ports and identity are not changed.</p>
      <input type="file" accept="application/json,.json" aria-label="Open an exported JSON file" @change="onFile" />
      <textarea v-model="importText" aria-label="Exported JSON" placeholder='{"version": 1, "mix": {...}}' style="margin-top: 0.5rem"></textarea>
      <div class="actions">
        <span class="msg" :class="importMsg.kind" style="margin: 0">{{ importMsg.text }}</span>
        <span class="spacer"></span>
        <button class="btn" :class="{ danger: armed }" :disabled="!importText.trim()" @click="doImport">{{ armed ? "Press again to replace the show" : "Import" }}</button>
      </div>
    </div>
  </div>
</template>
