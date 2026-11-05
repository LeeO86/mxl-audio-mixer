// What the console shows besides the strips (1.6.0 B5–B10): the meter scale and the dialogs a strip or bus
// menu opens over the mixer (a new strip or bus, operations, auto, auto-mixes). Per browser.
import { reactive, watch } from "vue";
import { store, stored } from "./store.js";

export const consoleUi = reactive({
  scale: stored("meter.scale", "dbfs"),
  panel: "", // "operations", "auto", "automix", "groups", "midi"
  add: "", // "strip" or "bus": the form for new ones
});
watch(
  () => consoleUi.scale,
  (v) => store("meter.scale", v),
);
