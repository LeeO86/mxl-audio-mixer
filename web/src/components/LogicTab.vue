<script setup>
// Logic graphs (§11): blocks wired together, evaluated at 100 Hz. This is the list editor; the
// cable canvas comes with the rest of the block types. Live values are polled while the tab is open.
import { computed, onMounted, onUnmounted, ref } from "vue";
import ConfirmButton from "./ConfirmButton.vue";
import { api } from "../api.js";
import { act, live, signals } from "../store.js";

const graphs = computed(() => live.mix?.logic || []);
const selected = ref("");
const graph = computed(() => graphs.value.find((g) => g.id === selected.value) || graphs.value[0] || null);
const name = ref("");
const values = ref({});
const buttons = ref([]);
let timer = 0;

const TYPES = [
  ["const", "Number"],
  ["button", "Button"],
  ["mute", "Mute state"],
  ["fader", "Fader"],
  ["vad", "Voice"],
  ["level", "Level"],
  ["and", "AND"],
  ["or", "OR"],
  ["not", "NOT"],
  ["compare", "Compare"],
  ["edge", "Rising edge"],
  ["latch", "Latch"],
  ["set-mute", "Set mute"],
  ["set-fader", "Set fader"],
];
const withSignal = ["mute", "fader", "vad", "level", "set-mute", "set-fader"];
const inputsOf = {
  and: ["a", "b"],
  or: ["a", "b"],
  compare: ["a", "b"],
  not: ["in"],
  edge: ["in"],
  latch: ["set", "reset"],
  "set-mute": ["in"],
  "set-fader": ["level"],
};

const addType = ref("const");
const addSignal = ref("");
const wire = ref({ from: "", to: "", port: "in" });

// One change at a time, each read from the server first: two quick adds would
// otherwise each send the list they saw, and the slower one would erase the other.
let queued = Promise.resolve();
function change(mutate) {
  const id = graph.value?.id;
  if (!id) return queued;
  queued = queued.then(async () => {
    const current = await api.get(`/api/v1/logic/${id}`).catch(() => null);
    if (!current) return;
    const body = mutate(current);
    if (body) await act(() => api.patch(`/api/v1/logic/${id}`, body));
  });
  return queued;
}

async function addGraph() {
  const body = {};
  if (name.value.trim()) body.name = name.value.trim();
  const made = await act(() => api.post("/api/v1/logic", body), "Logic graph added.");
  if (made) {
    name.value = "";
    selected.value = made.id;
  }
}

function addBlock() {
  const type = addType.value;
  const signal = addSignal.value;
  if (withSignal.includes(type) && !signal) return;
  change((g) => {
    const ids = new Set(g.blocks.map((b) => b.id));
    const id = [...Array(100)].map((_, i) => `${type}-${i + 1}`).find((x) => !ids.has(x));
    const block = { id, type, x: 40 + g.blocks.length * 16, y: 40 };
    if (withSignal.includes(type)) block.signal = signal;
    if (type === "const") block.value = 0;
    if (type === "compare") block.op = "gt";
    return { blocks: [...g.blocks, block] };
  });
}

function removeBlock(id) {
  change((g) => ({
    blocks: g.blocks.filter((b) => b.id !== id),
    wires: g.wires.filter((w) => w.from_block !== id && w.to_block !== id),
  }));
}

function setConst(b, value) {
  change((g) => ({ blocks: g.blocks.map((x) => (x.id === b.id ? { ...x, value: Number(value) } : x)) }));
}

function patch(body) {
  return change(() => body);
}

function addWire() {
  const { from, to, port } = wire.value;
  if (!from || !to || !port) return;
  change((g) => {
    const wires = g.wires.filter((w) => !(w.to_block === to && w.to_port === port));
    return { wires: [...wires, { from_block: from, from_port: "out", to_block: to, to_port: port }] };
  });
}

function removeWire(w) {
  change((g) => ({
    wires: g.wires.filter((x) => !(x.from_block === w.from_block && x.to_block === w.to_block && x.to_port === w.to_port)),
  }));
}

const portsOf = (id) => inputsOf[graph.value?.blocks.find((b) => b.id === id)?.type] || [];

async function refresh() {
  const g = graph.value;
  if (!g) return;
  const d = await api.get(`/api/v1/logic/${g.id}`).catch(() => null);
  if (d) {
    values.value = d.values || {};
    buttons.value = d.buttons || [];
  }
}

function press(block, pressed) {
  const g = graph.value;
  return act(() => api.put(`/api/v1/logic/${g.id}/buttons/${block.id}`, { pressed }));
}

function shown(v) {
  return v === undefined || v === null ? "" : Math.abs(v) >= 0.5 && Math.abs(v) <= 1 ? (v >= 0.5 ? "on" : "off") : Number(v).toFixed(1);
}

onMounted(() => {
  refresh();
  timer = setInterval(refresh, 250);
});
onUnmounted(() => clearInterval(timer));
</script>

<template>
  <div class="panel">
    <h3>Logic</h3>
    <p class="muted">Blocks run 100 times a second. A loop is allowed when an edge or a latch is in it, because those remember. Actions run when their input changes.</p>
    <div class="row">
      <select :value="graph?.id || ''" aria-label="Logic graph" @change="selected = $event.target.value">
        <option value="">graph…</option>
        <option v-for="g in graphs" :key="g.id" :value="g.id">{{ g.name }}</option>
      </select>
      <input v-model="name" placeholder="name" aria-label="New logic graph name" />
      <button class="btn small" @click="addGraph">+ Graph</button>
    </div>

    <template v-if="graph">
      <div class="row" style="margin-top: 0.6rem">
        <input :value="graph.name" aria-label="Logic name" @change="patch({ name: $event.target.value })" />
        <label class="check"><input type="checkbox" :checked="graph.enabled" @change="patch({ enabled: $event.target.checked })" /> on</label>
        <label class="check" title="values are computed, actions do not run"><input type="checkbox" :checked="graph.dry_run" @change="patch({ dry_run: $event.target.checked })" /> dry run</label>
        <ConfirmButton label="Remove" small @confirm="act(() => api.del(`/api/v1/logic/${graph.id}`))" />
      </div>

      <table class="amtable" style="margin-top: 0.6rem">
        <thead>
          <tr><th>Block</th><th>Type</th><th></th><th>Value</th><th></th></tr>
        </thead>
        <tbody>
          <tr v-for="b in graph.blocks" :key="b.id" :data-block="b.id">
            <td>{{ b.id }}</td>
            <td>{{ TYPES.find((t) => t[0] === b.type)?.[1] || b.type }}</td>
            <td>
              <template v-if="b.type === 'const'">
                <input type="number" :value="b.value" :aria-label="`${b.id} value`" style="width: 5rem" @change="setConst(b, $event.target.value)" />
              </template>
              <template v-else-if="b.signal">{{ signals.find((s) => s.id === b.signal)?.name || b.signal }}</template>
              <button v-else-if="b.type === 'button'" class="btn small" :class="{ primary: buttons.includes(b.id) }" :aria-pressed="buttons.includes(b.id)" @click="press(b, !buttons.includes(b.id))">
                {{ buttons.includes(b.id) ? "Pressed" : "Press" }}
              </button>
            </td>
            <td class="num" :data-value="b.id">{{ shown(values[b.id]) }}</td>
            <td><button class="btn small secondary" :aria-label="`Remove ${b.id}`" @click="removeBlock(b.id)">×</button></td>
          </tr>
        </tbody>
      </table>

      <div class="row" style="margin-top: 0.4rem">
        <select v-model="addType" aria-label="Block type">
          <option v-for="[id, label] in TYPES" :key="id" :value="id">{{ label }}</option>
        </select>
        <select v-if="withSignal.includes(addType)" v-model="addSignal" aria-label="Block signal">
          <option value="">signal…</option>
          <option v-for="s in signals" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
        <button class="btn small" @click="addBlock">+ Block</button>
      </div>

      <div class="caption" style="margin-top: 0.8rem">Wires</div>
      <div v-for="(w, i) in graph.wires" :key="i" class="row" :data-wire="`${w.from_block}-${w.to_block}-${w.to_port}`">
        <span>{{ w.from_block }} → {{ w.to_block }}.{{ w.to_port }}</span>
        <button class="btn small secondary" :aria-label="`Remove wire ${w.from_block} to ${w.to_block}`" @click="removeWire(w)">×</button>
      </div>
      <div class="row">
        <select v-model="wire.from" aria-label="Wire from">
          <option value="">from…</option>
          <option v-for="b in graph.blocks.filter((b) => !['set-mute', 'set-fader'].includes(b.type))" :key="b.id" :value="b.id">{{ b.id }}</option>
        </select>
        <select v-model="wire.to" aria-label="Wire to" @change="wire.port = portsOf(wire.to)[0] || ''">
          <option value="">to…</option>
          <option v-for="b in graph.blocks.filter((b) => inputsOf[b.type])" :key="b.id" :value="b.id">{{ b.id }}</option>
        </select>
        <select v-model="wire.port" aria-label="Wire port">
          <option v-for="p in portsOf(wire.to)" :key="p" :value="p">{{ p }}</option>
        </select>
        <button class="btn small" @click="addWire">+ Wire</button>
      </div>
    </template>
  </div>
</template>
