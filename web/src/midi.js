// Web MIDI control surfaces for the console (A7). The browser talks to the MIDI devices; the mixer sees only
// the same API calls the console makes (patchSignal, toggleSolo, select), so nothing changes on the server.
// Each input port has a mode, remembered in this browser by its name:
// - Mackie Control (MCU): 8 strips from the bank (the console's strips, then its buses), motor faders, mute,
//   solo (PFL), select, V-pot = pan, bank and channel buttons, the master fader on a chosen bus, names and
//   levels on the display. An extender (MCU XT) takes the next 8 strips.
// - Generic (learn): a control learnt on a fader, mute, PFL, AFL or pan of a strip or bus; the mappings are
//   kept in this browser and can be exported and imported as JSON.
// Feedback (motor faders, LEDs, V-pot rings) goes to the output port paired with the input (by default the
// one of the same name). Web MIDI needs a secure page (https or localhost).
import { reactive, watch } from "vue";
import { dbValue } from "./api.js";
import { MCU, bendToDb, controlKey, controlText, controlValue, dbToBend, feedback, lcd, lcdCell, lcdDb, panRing, parseControl, parseMcu, LEARN_PARAMS } from "./midicodec.js";
import { buses, live, patchSignal, select, signalOf, soloed, store, stored, strips, toggleSolo } from "./store.js";

function load(key, fallback) {
  try {
    return JSON.parse(stored(key, "")) ?? fallback;
  } catch {
    return fallback;
  }
}

export const midi = reactive({
  enabled: stored("midi.enabled", "") === "1",
  state: "off", // off, connecting, ready, insecure, unsupported, denied
  sysex: false,
  inputs: [], // { name, state }
  outputs: [],
  devices: load("midi.devices", {}), // input name -> { mode: "off" | "mcu" | "mcu-xt" | "generic", output: name }
  bank: 0, // the first strip of the Mackie Control bank
  master: stored("midi.master", ""), // the bus on the master fader; "" the first bus
  mappings: load("midi.mappings", []), // { device, control, target: { signal, param } }
  learning: false,
  target: null, // { signal, param } being learnt
  learnt: "", // the last mapping learnt, as text
});

/** Why Web MIDI cannot run on this page, or "". */
export function midiBlocked() {
  if (typeof window !== "undefined" && !window.isSecureContext) return "insecure";
  if (typeof navigator === "undefined" || !navigator.requestMIDIAccess) return "unsupported";
  return "";
}

let access = null;
const saveDevices = () => store("midi.devices", JSON.stringify(midi.devices));
const saveMappings = () => store("midi.mappings", JSON.stringify(midi.mappings));

/** Asks the browser for MIDI (with SysEx for the Mackie display; without when that is refused). */
export async function startMidi() {
  const blocked = midiBlocked();
  if (blocked) {
    midi.state = blocked;
    return false;
  }
  midi.state = "connecting";
  try {
    access = await navigator.requestMIDIAccess({ sysex: true });
    midi.sysex = true;
  } catch {
    try {
      access = await navigator.requestMIDIAccess();
      midi.sysex = false;
    } catch {
      midi.state = "denied";
      return false;
    }
  }
  midi.enabled = true;
  store("midi.enabled", "1");
  midi.state = "ready";
  access.onstatechange = ports;
  ports();
  return true;
}

export function stopMidi() {
  if (access) {
    for (const i of access.inputs.values()) i.onmidimessage = null;
    access.onstatechange = null;
  }
  access = null;
  midi.enabled = false;
  midi.state = "off";
  midi.inputs = [];
  midi.outputs = [];
  store("midi.enabled", "");
  stopLearn();
}

function ports() {
  if (!access) return;
  midi.inputs = [...access.inputs.values()].map((p) => ({ name: p.name, state: p.state }));
  midi.outputs = [...access.outputs.values()].map((p) => ({ name: p.name, state: p.state }));
  for (const input of access.inputs.values()) input.onmidimessage = (e) => onMessage(input.name, e.data);
  sent.clear();
  refresh();
}

/** The mode and output of an input port; an unknown port is off. */
export const deviceOf = (name) => midi.devices[name] || { mode: "off", output: name };
export function setDevice(name, change) {
  midi.devices[name] = { ...deviceOf(name), ...change };
  saveDevices();
  sent.clear();
  refresh();
}

function output(name) {
  const want = deviceOf(name).output;
  if (!access || !want) return null;
  return [...access.outputs.values()].find((o) => o.name === want) || null;
}

// ---- sending, with what each port shows -------------------------------------------------
const sent = new Map(); // "port|what" -> the last message, so only changes are sent
function send(port, what, data, force = false) {
  if (!port || !data) return;
  if (data[0] === 0xf0 && !midi.sysex) return;
  const key = `${port.name}|${what}`;
  const text = data.join(",");
  if (!force && sent.get(key) === text) return;
  sent.set(key, text);
  try {
    port.send(data);
  } catch {
    /* a port that went away: the state change lists it again */
  }
}

// A control the operator moves gets no feedback for a moment, so the server's slightly older value does not
// pull a motor fader back under the finger.
const HOLD_MS = 400;
const moved = new Map(); // "device|what" -> time
const held = (key) => performance.now() - (moved.get(key) || -Infinity) < HOLD_MS;
let after = 0;
function markMoved(key) {
  moved.set(key, performance.now());
  // When the hold is over, the latest value goes out once more.
  clearTimeout(after);
  after = setTimeout(() => refresh(), HOLD_MS + 50);
}
const touched = new Set(); // "device|strip"

// ---- Mackie Control ---------------------------------------------------------------------
/** The signals the Mackie Control banks run over: the console's strips, then its buses. */
export const surface = () => [...strips.value, ...buses.value];
const masterBus = () => buses.value.find((b) => b.id === midi.master) || buses.value[0] || null;
/** The first strip of a Mackie device: the bank, plus 8 per extender before it. */
function offsetOf(name) {
  if (deviceOf(name).mode !== "mcu-xt") return midi.bank;
  const xts = midi.inputs.map((i) => i.name).filter((n) => deviceOf(n).mode === "mcu-xt");
  return midi.bank + MCU.STRIPS * (xts.indexOf(name) + 1);
}
export function moveBank(by) {
  const last = Math.max(0, surface().length - MCU.STRIPS);
  midi.bank = Math.max(0, Math.min(last, midi.bank + by));
}
const hasPan = (s) => !!s && strips.value.includes(s) && (s.format === "mono" || s.format === "stereo");
const level = (db) => (Number.isFinite(db) ? db : "-inf");

function onMcu(name, data) {
  const m = parseMcu(data);
  if (!m) return;
  const off = offsetOf(name);
  const sig = m.strip === MCU.MASTER ? (deviceOf(name).mode === "mcu" ? masterBus() : null) : surface()[off + m.strip];
  if (m.kind === "bank") return moveBank(m.dir * MCU.STRIPS);
  if (m.kind === "channel") return moveBank(m.dir);
  if (m.kind === "touch") {
    const key = `${name}|${m.strip}`;
    if (m.on) touched.add(key);
    else {
      touched.delete(key);
      refresh(true);
    }
    return;
  }
  if (!sig) return;
  if (m.kind === "fader") {
    markMoved(`${name}|fader${m.strip}`);
    patchSignal(sig.id, { fader_db: level(bendToDb(m.value)) });
  } else if (m.kind === "vpot" && hasPan(sig)) {
    markMoved(`${name}|ring${m.strip}`);
    patchSignal(sig.id, { pan: Math.round(Math.max(-1, Math.min(1, (sig.pan || 0) + m.ticks * 0.02)) * 100) / 100 });
  } else if (m.kind === "button" && m.on) {
    if (m.button === "mute") patchSignal(sig.id, { mute: !sig.mute });
    else if (m.button === "solo") toggleSolo(sig.id, "pfl");
    else if (m.button === "select") select(sig.id);
    else if (m.button === "vpot_push" && hasPan(sig)) patchSignal(sig.id, { pan: 0 });
  }
}

function refreshMcu(name, force) {
  const port = output(name);
  if (!port) return;
  const mode = deviceOf(name).mode;
  const off = offsetOf(name);
  const list = surface();
  const top = [];
  const bottom = [];
  for (let i = 0; i < MCU.STRIPS; i++) {
    const s = list[off + i] || null;
    const db = s ? dbValue(s.fader_db) : -Infinity;
    if (!touched.has(`${name}|${i}`) && !held(`${name}|fader${i}`)) {
      const v = s ? dbToBend(db) : 0;
      send(port, `fader${i}`, [0xe0 | i, v & 0x7f, v >> 7], force);
    }
    send(port, `mute${i}`, [0x90, MCU.MUTE + i, s?.mute ? 0x7f : 0], force);
    send(port, `solo${i}`, [0x90, MCU.SOLO + i, s && soloed.value[s.id] ? 0x7f : 0], force);
    send(port, `select${i}`, [0x90, MCU.SELECT + i, s && live.selected === s.id ? 0x7f : 0], force);
    if (!held(`${name}|ring${i}`)) send(port, `ring${i}`, [0xb0, MCU.RING + i, hasPan(s) ? panRing(s.pan) : 0], force);
    top.push(lcdCell(s ? s.name : ""));
    bottom.push(lcdCell(s ? lcdDb(db) : ""));
  }
  send(port, "lcd-top", lcd(top.join(""), 0, mode === "mcu-xt"), force);
  send(port, "lcd-bottom", lcd(bottom.join(""), 56, mode === "mcu-xt"), force);
  if (mode === "mcu" && !touched.has(`${name}|${MCU.MASTER}`) && !held(`${name}|fader${MCU.MASTER}`)) {
    const b = masterBus();
    const v = b ? dbToBend(dbValue(b.fader_db)) : 0;
    send(port, "master", [0xe0 | MCU.MASTER, v & 0x7f, v >> 7], force);
  }
}

// ---- generic MIDI learn -----------------------------------------------------------------
/** A mapping's target as text ("Vox fader"). */
export const targetText = (t) => `${signalOf(t.signal)?.name || t.signal} ${LEARN_PARAMS[t.param]?.label || t.param}`;
export { controlText };

function valueOf(t) {
  const s = signalOf(t.signal);
  if (!s) return undefined;
  if (t.param === "fader") return dbValue(s.fader_db);
  if (t.param === "mute") return !!s.mute;
  if (t.param === "pfl" || t.param === "afl") return soloed.value[s.id] === t.param;
  if (t.param === "pan") return s.pan ?? 0;
  return undefined;
}

function onGeneric(name, data) {
  const parsed = parseControl(data);
  if (!parsed) return;
  const key = controlKey(parsed.control);
  if (midi.learning && midi.target) {
    if (!parsed.press) return; // learn on a press or a move, not on a release
    const mapping = { device: name, control: parsed.control, target: { ...midi.target } };
    midi.mappings = [...midi.mappings.filter((m) => !(m.device === name && controlKey(m.control) === key)), mapping];
    saveMappings();
    midi.learnt = `${controlText(parsed.control)} → ${targetText(mapping.target)}`;
    setTarget(null);
    return;
  }
  for (const m of midi.mappings) {
    if (m.device !== name || controlKey(m.control) !== key) continue;
    const s = signalOf(m.target.signal);
    if (!s) continue;
    const v = controlValue(m.target.param, parsed);
    if (!v) continue;
    if (v === "toggle") {
      if (m.target.param === "mute") patchSignal(s.id, { mute: !s.mute });
      else toggleSolo(s.id, m.target.param);
    } else {
      markMoved(`${name}|${key}`);
      patchSignal(s.id, v);
    }
  }
}

function refreshGeneric(name, force) {
  const port = output(name);
  if (!port) return;
  for (const m of midi.mappings) {
    if (m.device !== name) continue;
    const key = controlKey(m.control);
    if (held(`${name}|${key}`)) continue;
    const v = valueOf(m.target);
    if (v === undefined) continue;
    send(port, `${key}`, feedback(m.control, m.target.param, v), force);
  }
}

export function removeMapping(i) {
  midi.mappings = midi.mappings.filter((_, k) => k !== i);
  saveMappings();
}
export function setMappings(list) {
  midi.mappings = list;
  saveMappings();
  sent.clear();
  refresh();
}

// Learn mode: a click on a learnable control (data-learn, inside its strip's data-signal) picks it instead
// of acting on it; the next MIDI control moved on a generic device is mapped to it.
let targetEl = null;
function setTarget(t, el = null) {
  targetEl?.classList.remove("midi-target");
  targetEl = el;
  el?.classList.add("midi-target");
  midi.target = t;
}
function pick(e) {
  const el = e.target.closest?.("[data-learn]");
  const signal = el?.closest("[data-signal]")?.dataset.signal;
  if (!el || !signal) return;
  e.preventDefault();
  e.stopPropagation();
  if (e.type === "pointerdown") setTarget({ signal, param: el.dataset.learn }, el);
}
export function startLearn() {
  midi.learning = true;
  midi.learnt = "";
  document.body.classList.add("midi-learning");
  for (const t of ["pointerdown", "click", "dblclick"]) window.addEventListener(t, pick, true);
}
export function stopLearn() {
  midi.learning = false;
  setTarget(null);
  if (typeof document === "undefined") return;
  document.body.classList.remove("midi-learning");
  for (const t of ["pointerdown", "click", "dblclick"]) window.removeEventListener(t, pick, true);
}

// ---- dispatch and feedback ------------------------------------------------------------------
function onMessage(name, data) {
  const mode = deviceOf(name).mode;
  if (mode === "mcu" || mode === "mcu-xt") onMcu(name, data);
  else if (mode === "generic") onGeneric(name, data);
}

/** Sends what changed to every device (`force`: everything again). */
export function refresh(force = false) {
  if (!access) return;
  for (const { name } of midi.inputs) {
    const mode = deviceOf(name).mode;
    if (mode === "mcu" || mode === "mcu-xt") refreshMcu(name, force);
    else if (mode === "generic") refreshGeneric(name, force);
  }
}

watch(() => [live.mix, live.status?.solo, live.selected, midi.bank, midi.master, midi.mappings], () => refresh(), { flush: "post" });
watch(
  () => midi.master,
  (v) => store("midi.master", v),
);
