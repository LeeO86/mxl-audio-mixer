# mxl-audio-mixer — Specification

Status: Draft v0.3 (for implementation by Claude Code or Cursor in a new, empty repository)

Changelog:
- v0.2: NMOS comes from the shared Rust crate `mxl-nmos`, extracted from the Strom fork (§3.1). The extraction is milestone 0 (§19). NvNmos and nmos-cpp shims are no longer options.
- v0.3: lessons from running the sibling media functions on a Kubernetes MXL platform, October 2026:
  - MXL input timing and arrival lag (§4.2, §4.4); output commit size and stable flow ids (§4.3);
  - real-time rules for Kubernetes (§2, §18);
  - the shared preview contract for WebRTC monitoring and talkback (§10.1, §10.2, App. B.1);
  - operator-screen widgets (§12.6, App. B.2) and the family look of the sibling web UIs (§12.0);
  - tally (§10.3);
  - NMOS details for `mxl-nmos` (§3.1);
  - a release plan (§19);
  - the platform contract G1–G14 written out in full (Appendix A), so the spec is self-contained.
Repository: `LeeO86/mxl-audio-mixer` (name can still change)
License: MIT (like the sibling repos; check every dependency's licence against it)
Aligns with: the **media function guideline G1–G14 (v1.0.0 contract, Appendix A)** and the
sibling repos `LeeO86/mxl-decklink`, `mxl-st2110-gateway`, `mxl-fabrics-agent`,
`mxl-multiviewer`, `mxl-webrtc-monitor`, `mxl-test-player`, `mxl-srt-gateway`,
`mxl-color-corrector`, `mxl-replay`

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119.

---

## 1. Purpose and concept

`mxl-audio-mixer` is a broadcast audio engine built for MXL, not a conventional mixer
adapted to it. It combines a freely routable **audio matrix**, **dynamically built
channel strips**, professional **broadcast DSP**, **loudness and speech intelligence**,
**automatic mixing**, a **control room** with PFL/AFL and talkback, a visual **logic
editor**, **VCA/DCA** groups, and **master/slave** operation for remote production.

Core ideas (from the concept paper this spec is based on):

1. **Everything is a matrix.** Any input can contribute to any output at any level.
   Buses and outputs are sources too, so mix-minus, nested buses, monitoring and
   talkback paths are ordinary routing. Feedback loops are allowed only through an
   explicit block delay, so the graph stays causal and deterministic.
2. **Channel strips are processing graphs.** A strip is a configurable chain of
   processing nodes (EQ, compressor, de-esser, …) with insert/aux tap points anywhere.
   Nodes are added, removed and reordered at runtime.
3. **Processing belongs to the signal path,** not to a physical input: the same nodes
   can sit on an input, aux, group, bus, output, monitor path or N-1.
4. **N-1 is a first-class routing abstraction:** select channels → "Create N-1"; the
   result is a normal signal that can be processed and routed further.
5. **DSP, analysis/control and UI are separate layers.** Detectors produce
   information; the control engine turns information into parameter changes; the DSP
   engine applies them at block boundaries. The UI is just one client of the API.
6. **The operator sees a console; the engine sees a graph.** High-level operations
   (group, N-1, stereo pair, VCA) generate the graph underneath.

Out of scope for v1: hardware control surfaces (protocol hooks prepared, §12.4),
plug-in hosting (VST/LV2), immersive/object audio beyond 7.1.4 bus widths, sample
rates other than 48 kHz, recording.

---

## 2. Architecture

```
                 ┌───────────────────────────── UI / API ─────────────────────────────┐
                 │ Console · Control room · Graph/routing · Logic editor · Snapshots  │
                 │ REST + WebSocket · WebRTC (monitoring, talkback) · Remote link     │
                 └──────────────────────────────┬─────────────────────────────────────┘
                                         configuration / commands
                 ┌──────────────────────────────▼─────────────────────────────────────┐
                 │ CONTROL ENGINE (non-real-time threads)                             │
                 │ graph compiler · logic runtime · VAD · loudness · auto-mix ·       │
                 │ auto-level · ducking · automation · smoothing targets · snapshots  │
                 └──────────────────────────────┬─────────────────────────────────────┘
                              compiled plans + parameter messages (lock-free queues)
                 ┌──────────────────────────────▼─────────────────────────────────────┐
                 │ DSP ENGINE (real-time thread(s))                                   │
                 │ MXL readers → plan execution (gain, EQ, dynamics, delay, mix,      │
                 │ matrix, metering taps) → MXL writers, monitor/WebRTC taps          │
                 └──────────────────────────────┬─────────────────────────────────────┘
                                               MXL
                          MXL audio inputs ◄───┴───► MXL audio outputs
```

- **Real-time rule:** the DSP thread never allocates, locks, logs synchronously or does
  I/O other than MXL reads/writes. Communication with other threads uses lock-free
  single-producer/single-consumer queues and atomically swapped plan pointers
  (old plans freed on the control side).
- **Denormals:** the DSP thread sets flush-to-zero and denormals-are-zero (x86 MXCSR
  FTZ/DAZ, or the equivalent) at start; tests still cover denormal inputs.
- **Thread pools follow the container's CPU limit, not the host.** Rust thread pools
  (tokio, rayon, FFT libraries) size themselves to the host's cores by default. In a pod
  with a CPU limit that leads to CFS throttling, which stops *every* thread of the
  container for the rest of a 100 ms period, the DSP thread included. Size every pool
  from the cgroup CPU limit (`/sys/fs/cgroup/cpu.max`), and keep non-real-time work at
  least one core below the limit.
- **Plans:** the control engine compiles the routing/processing graph into an
  **execution plan** (topologically sorted node list, buffer assignments, latency
  compensation delays, matrix coefficients). A new plan replaces the running one at a
  **block boundary**; nodes that exist in both plans keep their DSP state (filter
  memories, envelopes) so unrelated paths are not disturbed.
- **Parameters:** changes are messages `{node, param, value, ramp}` applied at the next
  block; gains and filter coefficients are smoothed (ramp default 10 ms) to avoid clicks.

---

## 3. Technology

- **Rust** (stable, edition 2021 or newer). DSP code in safe Rust with explicit SIMD
  where it pays (`std::simd` when stable on the chosen toolchain, else `wide` or
  target-feature intrinsics), and a scalar reference path used by tests.
- **MXL:** the official MXL Rust bindings from `dmf-mxl/mxl` if available at the pinned
  revision; otherwise thin FFI bindings to the C API, generated with `bindgen`. MXL pin:
  `release/v1.1` at `218ddaa` (platform pin), built with
  `-DMXL_ENABLE_FABRICS_OFI=OFF`.
- **NMOS (IS-04 v1.3, IS-05 v1.2, BCP-007-03):** the shared Rust crate **`mxl-nmos`**
  (§3.1). No NvNmos, no nmos-cpp, no C++ toolchain in this repo.
- **WebRTC:** through the platform's **preview contract** (Appendix B.1). The mixer
  publishes monitor paths as Opus over RTSP to MediaMTX, which serves them by WHEP. It
  reads browser talkback that MediaMTX received by WHIP, over RTSP. Without a shared
  MediaMTX the mixer starts its own as a supervised child process. Opus via `libopus`;
  RTSP with a small Rust client or the MediaMTX API.
  - An in-process WebRTC stack (e.g. the sans-IO `str0m` crate) is allowed only if a
    measurement shows that the MediaMTX path cannot meet the monitoring latency (target
    ≤ 150 ms glass-to-ear in the browser). Record the measurement in
    `IMPLEMENTATION_PLAN.md`.
  - No GStreamer anywhere.
- **Web/API:** `axum` (or equivalent) for REST, WebSocket and static UI; UI in
  **Vue 3** embedded in the binary, no CDN. Node-graph editors (routing view, logic
  editor) with a maintained MIT-licensed Vue flow/graph library (e.g. Vue Flow).
- **Persistence:** show files and snapshots as JSON under `/config` (G1).
- Tests: `cargo test`, property tests (`proptest`) for DSP invariants, integration
  tests with libmxl and an nmos-cpp registry container.

### 3.1 NMOS via the shared crate `mxl-nmos`

- `mxl-nmos` is a pure-Rust NMOS node library, extracted from the NMOS node of the
  Strom fork (`LeeO86/strom`), which already implements IS-04 v1.3, IS-05 v1.2 and
  BCP-007-03, passes the AMWA BCP-007-03-01 suite and meets the platform contract
  (NMOS seed, DNS-SD off by default, announced IP, registration, deregistration on
  shutdown). It lives in its own repository (`LeeO86/mxl-nmos`) and is used by both
  Strom and this mixer.
- The mixer depends on a **tagged release** of `mxl-nmos` (git dependency with tag, or
  a private registry later); never on a branch.
- What the mixer needs from the crate (and therefore what the extraction MUST
  provide): one Node and Device; dynamic creation and removal of MXL audio senders
  (Source + Flow + Sender) and receivers at runtime; receiver activation delivered to
  the application **without restarting anything** (the application applies the new
  `mxl_domain_id`/`mxl_flow_id` to one receiver); IS-04 `subscription` updates;
  BCP-004-01 receiver capabilities; BCP-002-01 group hints; `NMOS_SEED`/UUIDv5 IDs,
  `NMOS_LABEL`, `NMOS_TAGS`; static registry with heartbeats and re-registration;
  readiness = registered and listed by the Query API; deregistration on shutdown;
  persistence hook for active connections (G9).
- Anything the mixer needs that the crate lacks is added **in `mxl-nmos`** (with tests
  and the AMWA suites there), not worked around in the mixer.
- CI of the mixer still runs the AMWA suites IS-04-01, IS-05-01, IS-05-02 and
  BCP-007-03-01 against the mixer itself.
- **NMOS behaviour learned on the platform** (the crate provides it; the mixer relies
  on it):
  - Scheduled IS-05 activations carry **TAI** timestamps. They are compared with
    CLOCK_TAI, not UTC.
  - An activation is accepted (staged → active) without waiting for the flow. A receiver
    whose flow does not exist yet goes to `waiting` and starts reading when the flow
    appears. The platform's fabrics agent mirrors flows to a node *because* a receiver
    there was activated, so the flow often appears after the activation.
  - `master_enable: false` is a normal activation (that receiver goes silent), not a
    reject.
  - BCP-007-03 allows a sender's `mxl_flow_id` to differ from its IS-04 Flow id. Never
    assume they are equal.
  - Readiness depends on registration; liveness never does. A restarting registry must
    not restart the pod.
  - AMWA in CI: receiver caps carry a version (BCP-004-01). With DNS-SD off, the tests
    that need DNS-SD cannot run; list them as expected "not tested" results instead of
    hiding them. Pull the testing-tool image from a mirror or with a Docker Hub login
    (rate limits broke CI runs of the sibling repos).

---

## 4. Signals, inputs, outputs, timing

### 4.1 Format

- 48 kHz, 32-bit float internally (64-bit accumulators for summing busses and
  loudness integration).
- **Block size** configurable: 48, 96, 144, 240 samples (1, 2, 3, 5 ms; default 96).
- Channel formats for strips and buses: mono, stereo, 3.0, 5.1, 7.1, 5.1.4, 7.1.4 (linked
  processing across a strip's channels).

### 4.2 MXL inputs (NMOS receivers)

- `MIXER_INPUT_FLOWS` audio receivers (default 16; each an `audio/float32` MXL flow
  with 1–64 channels). Each receiver has a label and group hint; activation accepted
  before the flow exists (`waiting`), IS-04 `subscription` updated; mirror domains
  resolved via the scan path (G2).
- **Input patch:** channels of input flows are patched to input strips (any channel to
  any strip, with linked groups for stereo/5.1). A flow format change (channel count)
  is handled without restart; unpatched channels are ignored, missing ones are silent
  and flagged.
- **Alignment and latency:** each input is read at
  `t_out − (MIXER_READ_OFFSET + input_extra_delay)`; `input_extra_delay` is per input.
  All inputs are therefore time-aligned on the TAI timeline. Late or missing samples
  are counted and replaced by silence with a short fade (no clicks).
  - **`auto` is the default for `input_extra_delay`.** The mixer measures each input's
    **arrival lag**: how far the flow's head index trails TAI, as a distribution, with
    the 99.9th percentile over a sliding window. It sets the delay to that value plus a
    margin, within a configurable maximum, and changes it only with a fade (or only on
    operator confirmation, configurable).
  - Manual stays possible. The lag is exported per input (§15), so the right value can
    be read off.
  - **Measured on the platform:**
    - Local flows arrive within about 1–5 ms.
    - Flows mirrored from another node by the fabrics agent (RDMA, about 210 samples
      per transfer) arrived up to 35 ms late on a test server and needed about 120 ms (6 video
      grains) on a three-node cluster.
    - ST 2110 gateway audio in MXL arrives in packet batches.

    A fixed 2 ms offset only works for local inputs.
  - **Read audio as late as possible:** the block's read happens just before its
    deadline, with a bounded wait for samples that are almost there. It does not happen
    at the start of the block period.
  - **Start-up:** the first windows after an input (re)opens are filled with silence and
    not counted as missing.
- **MXL flow life cycle** (known MXL behaviour at the pinned revision):
  - A flow is **deleted when its last writer releases it** and comes back with a new
    inode when the writer restarts. A reader on the old inode sees nothing. The mixer
    re-opens an input when its flow is re-created (inode or id change) and when the head
    index stops moving for a configurable time.
  - **Audio flows have no `lastWriteTime`.** Stall detection must watch head-index
    progress, not timestamps. A stalled input is counted, logged once per state change
    (`input_stalled` / `input_resumed`) and shown in the UI.
  - `FLOW_INVALID` is returned only with TOO_EARLY. Never treat it as a reason to give
    up an input.

### 4.3 MXL outputs (NMOS senders)

- Any bus, output, N-1, aux, group or monitor path can be published as an MXL audio
  flow (`audio/float32`, channel count = signal width or a configured layout). Each
  published signal is an NMOS sender (Source + Flow + Sender), group hint by signal
  name. Publishing/unpublishing at runtime creates/removes the sender; IDs derive from
  `NMOS_SEED` + stable signal ID, so they survive restarts (G3).
- Outputs are written into the mixer's own domain (G2) on the TAI timeline.
  - **Commit size is separate from the DSP block size:** `MIXER_OUTPUT_COMMIT_SAMPLES`
    (default = one block; per output overridable, e.g. 240 or 480).
    - Tiny commits are fine for local readers. But when the fabrics agent replicates an
      output to another node, every commit becomes a transfer. On RDMA (verbs), many
      small transfers caused receiver-not-ready errors and once only 4 % replication.
    - Test the chosen default with a mirrored output (§17).
- **Stable MXL ids:** the output domain id and every output's MXL flow id derive from
  `NMOS_SEED` + the stable signal id, like the NMOS ids. Downstream readers then keep
  receiving after a mixer restart or upgrade.
- Writers stay open across plan swaps. A published output's writer is closed only when
  the signal is unpublished or the mixer stops. Closing deletes the flow (see the life
  cycle above), and every downstream reader then has to re-open it.

### 4.4 Timing and latency

- The DSP loop is clocked by TAI: block n covers samples `[n·B, (n+1)·B)` of the
  output timeline; it starts when the input read deadline for that block is reached.
- **Latency budget (target):** `MIXER_READ_OFFSET` (default 2 ms) + one block + processing
  ≤ 5 ms end-to-end for default settings **with local inputs**, measured and exported
  (§15). Mirrored inputs add their measured arrival lag (§4.2). The UI shows each input's
  effective delay and the resulting latency per output.
- **TAI check at start:** all MXL timing is TAI. If CLOCK_TAI − CLOCK_REALTIME is not
  the current TAI−UTC offset (37 s since 2017), log an error and show a banner. Start
  anyway, but mark the timing as untrusted in `/readyz` details and the metrics. A host
  whose kernel TAI offset is unset puts every grain 37 s off.
- **Delay compensation:** nodes report their latency (e.g. look-ahead limiter); the
  plan compiler inserts compensating delays so that parallel paths summed into the
  same bus stay phase-coherent. Compensation can be disabled per bus (for minimum
  latency paths like talkback/IFB).
- **Feedback:** a cycle in the graph is only accepted if it contains an explicit
  `block delay` node (≥ 1 block); otherwise the compiler rejects the change with a
  clear error.

---

## 5. Routing model

- **Signals:** inputs, input strips, auxes, groups/subgroups, buses, N-1s, outputs,
  monitor paths, talkback, oscillator. Every signal can feed the matrix.
- **Matrix:** any signal → any summing point at a gain (−∞…+10 dB), with mute and
  phase; pre/post tap selectable per send (tap points: input, pre-EQ, post-EQ,
  pre-dynamics, post-dynamics, pre-fader, post-fader, post-pan, or after any node).
- **Width mapping:** mono→stereo (pan law selectable: −3 dB default, −4.5, −6, 0),
  stereo→mono (sum −3 dB or −6 dB), up/down-mix presets for 5.1/7.1.
- **High-level operations** that generate the graph:
  - **Group:** selected strips → new subgroup bus with routing.
  - **N-1:** selected strips → N-1 bus per selected participant (each receives the sum
    of all others) or one N-1 of the selection; result is a normal signal.
  - **Stereo pair / surround link:** link strips (shared processing and fader).
  - **Mix-minus from bus:** for any bus, generate "bus minus X".
- **Graph view:** a visual routing graph (read-write) for advanced users; the console
  view stays the default.

---

## 6. DSP nodes

All nodes: bypass, per-node latency report, metering tap (in/out), linked operation
across multichannel signals, parameter smoothing.

| Category | Nodes |
| --- | --- |
| Basic | gain/trim, fader, pan/balance/width, phase invert, delay (samples, ms, frames), mute, polarity per channel, summing |
| EQ | parametric EQ (up to 8 bands: bell, shelf, notch), high-pass/low-pass (12–48 dB/oct), **dynamic EQ** (per-band threshold/ratio/attack/release) |
| Dynamics | compressor (feed-forward, soft knee, sidechain with filter), limiter (look-ahead, true-peak option), expander, gate (with hysteresis, hold), **de-esser** (split-band and wide-band modes, frequency 2–12 kHz, threshold, range, listen/sidechain monitor), ducker (keyed by any signal or by VAD), multiband dynamics (3–5 bands, linear-phase crossover option) |
| Loudness | auto-level (speech-aware loudness controller, §8.2), loudness normaliser for outputs |
| Utility | oscillator/tone (sine, pink/white noise, sweep, EBU stereo ident), block delay (feedback), meter tap |

- Filter design: biquads/state-variable filters with coefficient smoothing; designs
  documented with references; verified against reference responses (§14).
- Compressor/limiter/de-esser: documented detector types (peak/RMS), ballistics,
  and gain-reduction metering exposed to the UI.

---

## 7. Metering and measurement

- Per signal: sample peak, **true peak** (ITU-R BS.1770 oversampling), RMS, PPM
  (selectable scales: EBU, Nordic, BBC, DIN), gain reduction per dynamics node.
- **Loudness** per configurable signal (outputs, buses, any monitor point):
  momentary, short-term, integrated, Loudness Range, max true peak, per ITU-R BS.1770
  and EBU R 128 (target −23 LUFS, tolerances configurable); start/stop/reset of
  integration from UI, logic or API.
- Spectrum analyser (FFT, 1/3- or 1/6-octave display) on any tap point (computed on the
  control side from tapped blocks, never in the DSP thread).
- Phase correlation and goniometer for stereo signals.
- Meters are sent to the UI over WebSocket at ≈ 25 Hz (configurable), decimated per
  client subscription.

---

## 8. Intelligence (control engine)

### 8.1 Voice activity detection

- Per selected signal, a VAD producing `speech_active`, `speech_probability`,
  `speech_level`, `speech_onset`, `speech_offset`. v1: a lightweight, CPU-cheap
  algorithm (spectral and energy features with hysteresis); the interface allows a
  model-based detector later. Runs on the control side from tapped audio, with a
  documented detection latency.

### 8.2 Speech-aware loudness (auto-level)

- Controller with a loudness target (e.g. −23 LUFS short-term) and a configurable
  **speech offset** (e.g. +2 LU) while speech is active; attack/release and max gain
  range; freeze when no signal. Implemented as a control algorithm that drives a gain
  node, not as hard-coded DSP.
  - also integrate the following function from this repo: https://github.com/andyweiss/Com-IS-Assist

### 8.3 Automatic mixing

- Gain-sharing auto-mixer (Dugan-style): for the selected channels, each channel's
  gain follows its share of the total level, with last-mic-hold, NOM attenuation,
  per-channel weight and bypass. Optional VAD weighting.
- Implemented as a controller producing gains for the matrix/fader nodes, so operator
  fader moves combine with it (auto-mix gain × operator gain).

### 8.4 Ducking and speech priority

- Duck any signal (music, atmos) under speech detected on other signals, with depth,
  attack, hold, release.

---

## 9. VCA / DCA groups

- **VCA (DCA) groups** control member faders without summing audio: effective member
  gain = member fader + sum of all assigned VCA offsets (dB), nested VCAs allowed
  (cycles rejected).
- VCA mute = mutes members (without changing their mute buttons' own state); VCA
  solo/PFL = PFL of all members.
- **Spill:** selecting a VCA shows its members on the console.
- **Mute groups** (separate from VCAs), with safe/isolate per channel.
- VCAs are controllable from logic, snapshots and the remote link like any fader.

---

## 10. Control room

The control room is a dedicated part of the engine and UI, not just another bus.

### 10.1 Monitoring

- **Monitor sources:** selectable per monitor path from programme bus, any bus/aux/N-1/
  output, any input, external MXL inputs ("off-air", "return"), oscillator. Source
  selection via buttons (configurable source list) and from logic.
- **Solo system:** PFL, AFL (stereo, post-pan), and optional SIP (solo-in-place,
  destructive; requires explicit enable and a solo-safe list). Modes: additive or
  interlock; PFL level trim; automatic PFL dim of the main monitor. PFL on any signal,
  including buses, N-1s, VCAs (members) and outputs.
- **Speaker sets:** main, alt, near-field (each an output signal with its own trim,
  delay, EQ for room alignment and bass management option), plus headphones.
- Monitor controls: level, dim (configurable depth), cut, mono check, phase check
  (L−R), L/R swap, solo L / solo R, reference level preset (e.g. 79/82/85 dB SPL cal),
  talkback dim.
- **Monitor transport:**
  - **MXL:** each speaker set/headphone path is published as an MXL audio flow (NMOS
    sender), so it can be routed via mxl-decklink or the ST 2110 gateway to physical
    speakers.
  - **WebRTC:** any monitor path can be listened to in the browser (WHEP, Opus,
    stereo, low-latency settings), through the preview contract (Appendix B.1): stream
    `<PREVIEW_PATH_PREFIX>/<monitor id>`. **Personal monitoring:** each logged-in UI
    client can have its own PFL and headphone selection without affecting the control
    room speakers (per-client monitor path, limited by `MIXER_MAX_WEB_MONITORS`).
    - Only paths someone listens to are encoded. MediaMTX's API (`MEDIAMTX_API_URL`)
      shows the readers, and an unread path stops encoding after a grace time.

### 10.2 Talkback and communication

- **Talkback:** talkback microphone from an MXL input or from the **browser microphone
  via WebRTC (WHIP)**; talkback buttons route it to selectable destinations (studio,
  IFB/N-1 feeds, remote, auxes) with dim of the control room; latching or momentary.
  - Browser talkback goes WHIP → MediaMTX (`<PREVIEW_PATH_PREFIX>/talkback-<client>`).
    The mixer reads it there over RTSP and decodes the Opus into a talkback signal with
    its own measured delay.
  - Browsers allow microphone access only on HTTPS origins. The platform serves the UI
    over HTTPS; for local tests, use `localhost`.
- **IFB/N-1 interrupt:** talkback can interrupt or mix into N-1 feeds, with per-feed
  priority.
- **Oscillator** to any destination for line-up (tone presets, EBU ident).
- **Listen-back** of any returning feed (e.g. remote return) on the monitor.

### 10.3 Tally and audio-follows-video

- The platform runs a tally calculator that sends **TSL UMD 5.0**. Each target gets
  its own source indexes; the calculator follows the signal path through the vision
  mixers. Details:
  - UDP (default port 8910) or TCP (8911) with DLE/STX framing;
  - SCREEN = target screen; INDEX = source index;
  - left lamp (LH) = red = on air; right lamp (RH) = green = on preview;
  - labels in UTF-16;
  - a full state after every change and at least once per second.
- The mixer has a TSL 5.0 receiver (`TSL_ENABLE`, `TSL_UDP_PORT`, `TSL_TCP_PORT`,
  `TSL_SCREEN`, `TSL_MAP`, as in mxl-multiviewer and mxl-webrtc-monitor):
  - tally is available to the logic editor (§11);
  - **audio-follows-video (AFV)** is a per-strip option: the strip opens (fade, depth and
    times configurable) when its mapped tally index is on air (LH), and closes
    otherwise.
- A vision mixer may do AFV for its own programme audio. The mixer's AFV is for
  productions where the audio console owns the programme mix.

---

## 11. Logic editor ("if this then that")

### 11.1 Model

- A **logic graph** of blocks (rectangles) with typed ports, connected by cables drawn
  in the UI. Signal types: boolean, number, level (dB), event (trigger), string.
- Blocks:
  - **Inputs/triggers:** UI button (soft key), fader position, mute/PFL state, meter
    level (with threshold and hysteresis), loudness value, VAD outputs, tally (TSL 5.0
    input), timer/clock (TAI-based), schedule (time of day), NMOS connection state,
    remote-link state, webhook/HTTP endpoint.
  - **Logic:** AND, OR, NOT, XOR, latch/flip-flop, toggle, edge detect, delay,
    debounce, timer/pulse, compare, math (add, scale, map/curve, min/max), select/mux,
    counter.
  - **Actions:** mute/unmute, set fader/VCA (with ramp), set any node parameter, PFL,
    talkback on/off, recall snapshot, start/reset loudness measurement, change monitor
    source, publish/unpublish signal, TSL tally out, HTTP webhook out, log/marker.
- Each graph can be enabled/disabled, has a name and description, and is saved in the
  show file.

### 11.2 Execution

- Runs in the control engine at a fixed tick (default 100 Hz) on a snapshot of current
  state; actions become parameter messages to the DSP engine. Deterministic evaluation
  order (topological); combinational loops are rejected unless they contain a
  latch/delay block.
- Every block shows its live value in the editor (debug view); a dry-run mode logs
  actions without executing them.

### 11.3 UI

- Canvas with drag-and-drop blocks from a palette, cables by dragging between ports,
  zoom/pan, grouping into sub-graphs (macros) with exposed ports, copy/paste,
  undo/redo, validation markers on invalid wiring.

---

## 12. User interface

### 12.0 Family look (UI harmonisation)

The mixer's web UI looks and behaves like the other LeeO86 media functions. Reference
implementations, all public; copy their CSS base and components instead of designing
new ones:
- `mxl-color-corrector` 1.1.x and `mxl-webrtc-monitor` 1.3.x (`web/src`);
- `mxl-replay` 1.4.x, `mxl-multiviewer` 1.4.x, `mxl-srt-gateway` 1.4.x,
  `mxl-test-player` 1.2.x.

- **Header:** node label, the function name and version, and state **pills**: running /
  degraded, NMOS registration, live updates connected, timing trusted (§4.4), DSP load.
- **Banners** at the top for: API unreachable, live updates (WebSocket) lost, a failed
  action (with the server's error text), timing untrusted.
- **Tabs with their own address** (`#console`, `#control-room`, `#routing`, `#logic`,
  `#snapshots`, `#loudness`, `#nmos`, `#status`, `#settings`), so links and reloads
  land on the same view.
- **Light and dark theme** (follows the system, switchable, remembered), with the
  shared colour tokens.
- **Status tab:** the probes (`/livez`, `/readyz` with details), the main `/metrics`
  counters, input arrival lag and state, MXL and NMOS state.
- **Settings tab:** every setting with its value and origin (`environment`, `file`,
  `default`) from `GET /api/v1/config`, plus export and import (G10).
- **Destructive actions** (reset, delete, unpublish, snapshot overwrite) need a second
  press within 3 s; **per-control reset** where a control has a default.
- **Touch first:** every control works with mouse, keyboard and touch (pointer
  events); faders and knobs have fine steps with − and + buttons that repeat while held
  (as the RGB pots of mxl-color-corrector 1.1.0).
- The UI is embedded in the binary (no CDN). Asset paths are absolute (`/assets/…`) so
  that pages under `/widget/` load them.

### 12.1 Console (default view)

- Channel strips with: input select/patch, trim, phase, processing chain overview
  (miniature EQ curve, gain-reduction meters per dynamics node), aux sends, pan, fader,
  mute, PFL/AFL, VCA assignment, meter, name/colour. Strip layers/banks; custom
  layouts (user-defined strip order and pages); group/bus/VCA/master sections.
- Touch-friendly, works on tablets; multiple simultaneous clients stay in sync.

### 12.2 Processing chain editor

- Select any signal → its chain is shown; drag processing nodes from a palette into
  the chain ("I want an EQ here"), reorder by drag, bypass, detail editors for each
  node (EQ curve with handles, compressor transfer curve, de-esser with listen).

### 12.3 Other views

Control room (§10), routing matrix (grid of sources × destinations), graph view (§5),
logic editor (§11), snapshots, loudness and metering bridge, NMOS (receivers, senders,
input patch), remote link (§13), settings (G1, G10).

### 12.4 External control (prepared)

- The REST/WebSocket API covers every operator action, so other UIs or automation can
  drive the mixer. Hardware surfaces (e.g. MIDI/HUI, Ember+, OSC) are not implemented
  in v1, but the parameter addressing scheme (stable IDs for every parameter) MUST make
  such mappings possible later; document it.

### 12.5 Snapshots and show files

- **Show file** = complete configuration (graph, strips, routing, logic, layouts,
  control room). **Snapshots** = parameter states within a show, with recall filters
  (per channel/section/parameter class) and safes, crossfade time for faders.
- Auto-save, versioned history, export/import (G10).

### 12.6 Widgets for operator screens

The platform's production designer builds operator screens from widgets of several
functions. The mixer offers widgets by the contract in Appendix B.2. Proposed set (the
ids are part of the contract once released):

| id | Params | Shows and controls | min_size |
| --- | --- | --- | --- |
| `strip` | `{strip}` | one channel strip: fader, mute, PFL, meter, gain reduction, name | 120×480 |
| `fader-bank` | `{bank}` or `{strips}` (list) | a bank of strips, as on the console | 480×480 |
| `loudness` | `{signal}` | momentary / short-term / integrated LUFS, LRA, true peak, start/reset | 320×200 |
| `monitor` | `{monitor}` | control-room source select, level, dim, cut, mono | 360×200 |
| `talkback` | `{destinations?}` | talkback buttons (momentary/latching), with browser mic | 260×110 |
| `vca` | `{vca}` | one VCA fader with mute and spill | 120×480 |

---

## 13. Master/slave and remote production

### 13.1 Use case

A **far-side mixer** runs at the remote venue (its own mixer instance on that site's
platform, local DSP, local inputs, local N-1/IFB). The **home mixer** controls it, so
the operator mixes the remote side from the home control room while the audio is
processed where it originates (low latency for local returns, only the result is sent
home, e.g. via the SRT gateway).

### 13.2 Model

- An instance can be **master**, **slave**, or both (master of some, slave to one).
- A master links to one or more slaves over a **remote control link**:
  - control: parameter changes, snapshot recall, logic actions, routing changes,
    with sequence numbers and acknowledgements;
  - state: the slave reports its full state on connect and deltas afterwards;
  - meters: decimated meters (e.g. 10–25 Hz) and loudness values back to the master;
  - audio: **not** carried by the link (media goes via MXL/SRT); optional low-bitrate
    WebRTC listen path for PFL of slave signals.
- **Scope:** the whole slave mixer, or selected strips/sections mapped into the
  master's console as a **remote layer** (marked visually).
- **Transport:** WebSocket over TLS (WAN-capable), authenticated with a pre-shared token
  or mutual TLS (configurable; secrets are never exported, G1/G10). Designed for WAN
  latency and loss: no blocking on acks, last-writer-wins per parameter with sequence
  numbers.
- **Failure behaviour:** on link loss the slave **holds its last state** (fail-safe),
  shows "master lost" and allows **local takeover** by a local operator (configurable:
  allowed/denied/requires PIN). On reconnect, the master sees the diff and chooses
  "push master state" or "adopt slave state".
- **Arbitration:** per strip/section ownership (master or local); conflicting changes
  are rejected with a visible reason.
- Link health in UI and metrics: RTT, lost updates, last-sync time.

### 13.3 v1 scope

One master ↔ up to 4 slaves, full parameter control, state sync, meters, takeover and
re-sync. Redundant "tracking" (two instances processing the same inputs in lock-step)
is a later candidate.

v1's link (README "Remote production"; IMPLEMENTATION_PLAN X5–X10):

- the master connects with `wss://`, and a TLS-terminating proxy sits in front of
  the slave's web port; `ws://` (token in clear) is for a trusted network only;
- one master per slave at a time;
- each side pings every second and drops a peer it has not heard for 5 s; the
  master then reconnects;
- commands are numbered and acknowledged per connection, at most once (not resent);
- a message type a peer does not know is ignored, so versions interoperate.

---

## 14. Platform contract (G1–G14)

The mixer MUST meet the media function guideline G1–G14 like the sibling repos and
document the status per item in its v1.0.0 release notes. The full text is in
**Appendix A**, and the later platform conventions (preview contract, widgets, image
labels, real-time) in **Appendix B**. These appendices are the contract. In particular:

- G1 configuration (env > file > `/config` > defaults; invalid → exit 78; no secrets
  in exported files; remote-link secrets via env/Secret only);
- G2 MXL domains (scan path `/Volumes/mxl`, which must exist; own output domain
  directly below it, created once; an existing different id is kept with a warning,
  never overwritten; `MXL_HISTORY_DURATION_NS`);
- G3 NMOS identity (`NMOS_SEED` UUIDv5 for all resources, `NMOS_LABEL`, `NMOS_TAGS`,
  group hints);
- G4 static registry, `NMOS_DNS_SD=false` by default; G5 `NMOS_HOST_ADDRESS` (pod IP);
- G6 ports by env: `WEB_PORT` default **8190**, `NMOS_PORT` default **3342** (+1 for
  the events WebSocket if used). The built-in MediaMTX (only when `PREVIEW_PUBLISH_URL`
  is unset, Appendix B.1) uses its own port block:
  - `MEDIAMTX_RTSP_PORT` 8954 (127.0.0.1);
  - `MEDIAMTX_WHEP_PORT` 8599 (WHEP and WHIP);
  - `MEDIAMTX_ICE_UDP_PORT` 8199;
  - API off.

  These ports are free on the platform today. Bind failure → 75;
- G7 `/livez`, `/readyz`, `/metrics` (prefix `mxl_audio_mixer_`);
- G8 SIGTERM: fade outputs, release MXL, deregister, optional own-domain cleanup,
  exit 143 within `SHUTDOWN_TIMEOUT_S`;
- G9 IS-05/BCP-007-03 incl. persistence of active connections across restarts;
- G10 `GET /api/v1/config/export`, `POST /api/v1/config/import` (show file);
- G11 image as uid 1000, tags `X.Y.Z`/`X.Y`/`X`, `git-<sha7>`, `nightly-dev`, label
  `io.dmf.mxl.revision`; G12 Kubernetes example on the pod network; G13 docs; G14 tests.

---

## 15. Metrics (prefix `mxl_audio_mixer_`)

`dsp_block_seconds` (histogram), `dsp_deadline_misses_total`, `dsp_load_ratio`
(block time / block period), `plan_swaps_total`, `plan_compile_seconds`,
`input_late_samples_total{input}`, `input_lag_ms{input}`, `output_blocks_total`,
`signals`, `nodes{type}`, `loudness_lufs{signal,window}`, `true_peak_dbtp{signal}`,
`vad_active{signal}`, `remote_link_up{peer}`,
`remote_link_rtt_ms{peer}`, `logic_ticks_total`, `logic_actions_total{graph}`,
`dsp_realtime` (0/1: the DSP thread runs `SCHED_FIFO`), process CPU/memory.
The browsers' WebRTC sessions are counted by MediaMTX, not the mixer:
`preview_readers{path}` (below) is their number per stream, read from a shared
MediaMTX's API (the built-in one runs with its API off).

Also:
- `input_arrival_lag_ms{input}` (histogram or p50/p99/p999 gauges);
- `input_effective_delay_ms{input}`;
- `input_state{input}` (0 unrouted, 1 waiting, 2 running, 3 stalled);
- `input_reopens_total{input}`, `input_missing_samples_total{input}`,
  `input_overrun_samples_total{input}` (the ring had already overwritten the
  samples; `input_late_samples_total` is the source that had not written them);
- `output_commit_lag_ms{output}`;
- `timing_trusted` (0/1, §4.4);
- `preview_state{path}` and `preview_readers{path}` (Appendix B.1);
- `tally_packets_total`.

Grafana dashboard in `deploy/grafana/`.

---

## 16. Performance targets (measure and record)

- Precision 3930 class CPU, one DSP thread: 64 mono input strips each with HPF,
  4-band EQ, compressor, de-esser on 16 of them; 32 buses; 16 auxes; 8 N-1s; 4 VCAs;
  metering on all signals; block 96 samples: `dsp_load_ratio` < 0.5, zero deadline
  misses over 1 h.
- Optional second DSP thread for independent subgraphs (planner partitions the graph)
  if a single thread is not enough; document the result.
- End-to-end latency (MXL input sample → MXL output sample) at default settings
  ≤ 5 ms, measured with a click test.

---

## 17. Testing

- **DSP unit tests:** frequency responses of EQ/filters against reference designs
  (tolerance documented), compressor/limiter/de-esser static curves and time
  constants, true-peak and loudness against the **EBU Tech 3341 and 3342** test
  signals (must pass), pan laws, summing accuracy, denormal handling.
- **Engine tests:** plan swap at block boundary without discontinuities (sample
  continuity check), state preserved for unchanged nodes, delay compensation keeps
  parallel paths phase-coherent (null test), feedback without delay rejected.
- **Logic tests:** evaluation order, loop rejection, latch/timer behaviour, actions.
- **Remote link tests:** two instances, link loss → hold → takeover → reconnect →
  re-sync both directions.
- **Integration (CI):** libmxl with test writers (tone per channel), nmos-cpp registry,
  IS-05 activation before the flow exists → `waiting` → running; N-1 output contains
  all but own channel (correlation check); WebRTC WHEP session established
  (headless client) and receives audio; AMWA IS-04-01, IS-05-01, IS-05-02,
  BCP-007-03-01.
- **MXL life-cycle tests:**
  - the writer of an input restarts (flow deleted, re-created with a new inode) → the
    input re-opens by itself;
  - a stalled writer → `input_stalled` and the state, then resumed;
  - an activation before the flow exists → `waiting` → running;
  - the mixer restarts → output flow ids unchanged, and a downstream reader keeps
    receiving.
- **Arrival-lag test:** an input written late on purpose (writer delayed by 20–40 ms) →
  `auto` delay settles, 0 missing samples afterwards.
- **Preview contract:** own MediaMTX (child started, path published, killed → restarted
  and published again) and shared MediaMTX (`PREVIEW_PUBLISH_URL` set, no child); a
  headless WHEP client receives audio; WHIP talkback reaches a strip.
- **Widgets:** `/widgets` JSON and CORS (listed origin only), widget pages with the CSP
  and no `X-Frame-Options`, 400/404/405; a framing check in a headless browser from an
  allowed and a refused origin.
- **Mirrored outputs (a multi-node setup, not CI; run by the maintainers):** an output replicated by mxl-fabrics-agent to a
  second node at the default commit size: full sample rate on the far side, 0 gaps.
- **Load test** for §16, documented, including CPU throttling counters
  (`nr_throttled` in `cpu.stat`) under a CPU limit.

---

## 18. Deployment

Pod network; MXL root `hostPath`; uid 1000; `/config` volume for show files.
- **WebRTC:** through the shared MediaMTX (`PREVIEW_*`, Appendix B.1), so normally no
  hostPort. Only the built-in MediaMTX fallback needs its ICE UDP port reachable
  (hostPort, plus the node IP in its ICE candidates, as mxl-webrtc-monitor does).
- **Real time:**
  - Scheduling is optional (`MIXER_RT_PRIORITY`, default off, needs `CAP_SYS_NICE`).
    The value MUST be **≤ 49**: the kernel's IRQ threads run at SCHED_FIFO 50, and a DSP
    thread above them starves interrupt handling (the platform runs its real-time
    functions at 49). Kubernetes `capabilities.add` only puts `SYS_NICE` into the
    bounding set of a non-root process, so the image carries it as a permitted file
    capability (`setcap cap_sys_nice+p`) and the mixer raises it into its effective set
    before any thread starts. Without it the mixer still starts (`SCHED_OTHER`, a
    warning, `dsp_realtime` 0).
  - **Guaranteed QoS with whole CPUs** (static CPU manager, exclusive cores) is the
    supported way to run the DSP thread on the platform. A Burstable pod with a CPU
    limit gets CFS throttling (§2).
  - Pin the DSP thread to one of the pod's CPUs, read from the affinity mask; never
    assume CPU 0 or any fixed number. The platform's static CPU manager gives pods
    arbitrary CPU sets. The DSP CPU is the last one whose SMT siblings are all in the
    pod (a whole core); request an even number of CPUs: 4 preferred; 2 is the minimum
    (not measured), and then the non-DSP threads run on the DSP CPU's sibling.
- **Compose demo:** registry, mxl-test-player (tones), the mixer, mxl-webrtc-monitor,
  MediaMTX; plus a two-instance demo for master/slave.
- **Kubernetes Deployment** that `mxl-poc-platform` can vendor (L3 production function).

---

## 19. Implementation order

0. **Milestone 0 — prerequisites (gate before any DSP work):**
   - `mxl-nmos` extracted from the Strom fork and released (separate prompt/repo);
     Strom switched to it with its NMOS and AMWA tests still green;
   - spike in this repo: `mxl-nmos` node with one MXL audio receiver and one sender
     on a real flow, passing AMWA IS-04-01, IS-05-01, IS-05-02, BCP-007-03-01;
   - spike: MXL `audio/float32` read and write from Rust on the pinned revision
     `218ddaa`, on the TAI block clock, with a measured block jitter.
   Record the results in `IMPLEMENTATION_PLAN.md` before continuing.
1. Skeleton, config, ops endpoints, metrics, CI, image; MXL I/O with TAI block loop.
2. Plan compiler and DSP core: gain, fader, pan, mute, matrix, buses, metering;
   NMOS receivers/senders; input patch.
3. EQ, filters, dynamics, de-esser, delay; delay compensation; true peak and loudness
   (EBU 3341/3342 tests).
4. Console UI, processing chain editor, routing matrix; snapshots and show files.
5. Control room: PFL/AFL, monitor paths, MXL monitor outputs, WebRTC monitoring and
   talkback.
6. VCA/DCA, mute groups, N-1 and group operations.
7. Control engine features: VAD, auto-level, auto-mix, ducking.
8. Logic editor and runtime.
9. Remote link master/slave.
10. Performance work and load tests.

**Release plan.** The scope is large. Each release is usable on its own, tested on hardware by the maintainers,
and documented for the platform (image digest, settings, ports, widgets; Appendix B.3):
- **1.0.0:** milestones 0–6 (matrix, DSP, console, control room with MXL and WebRTC
  monitoring and talkback, VCA, N-1), plus widgets `strip`, `fader-bank`, `loudness`,
  `monitor`, `talkback`, and tally/AFV.
- **1.1.0:** milestone 7 (VAD, auto-level, auto-mix, ducking).
- **1.2.0:** milestone 8 (logic editor).
- **1.3.0:** milestone 9 (master/slave).

Performance (milestone 10) is measured at every release, not only at the end.

## 20. Open points

- Channel and bus counts required for the first productions (sizing of §16).
- Which hardware control surface to support first (stage 2).
- Whether talkback from browsers needs authentication/roles.
- Real-time scheduling permissions on the platform nodes (`CAP_SYS_NICE`, priority ≤ 49).
- Default commit size for mirrored outputs (§4.3), to be measured with mxl-fabrics-agent.
- Whether the MediaMTX path meets the browser monitoring latency (§3); if not, the
  in-process WebRTC option.

---

## Appendix A — Platform contract G1–G14 (v1.0.0)

The target platform deploys the media functions on Kubernetes (RKE2) with Helm, pinned
by image digest. This appendix is the
contract as the sibling repos implemented it for their v1.0.0 releases; their READMEs
and specifications show working examples.

### A.0 How the platform runs media functions

- **Shared MXL root:** one tmpfs per node at `/Volumes/mxl`, mounted at the same path
  into every container (hostPath). It is the parent of the domain directories.
  - Every instance gets its own output domain directory `/Volumes/mxl/<instance>` and a
    stable domain id from the platform.
  - The fabrics agent mirrors flows between nodes as `mirror-*` sibling directories.
- **Identity:** uid/gid 1000, `supplementalGroups: [1000]`, no `hostIPC`.
- **Networking:** the pod network by default. Several instances of one function can run
  on the same node, each with its own ports.
- **NMOS:**
  - one static registry per platform: the Registration API on port P, the Query API on
    P+1, the Query WebSocket on P+2; DNS-SD is not used anywhere;
  - each instance gets `NMOS_SEED=<production>-<function>`, the same node label, and
    tags naming its production and function;
  - connections are made with IS-05 and BCP-007-03 (`mxl_domain_id`, `mxl_flow_id`).
- **Lifecycle:**
  - `production-up` waits for readiness, then makes the IS-05 connections;
  - `production-down` disables the receivers, sends SIGTERM, then checks that the
    function's NMOS node and MXL domain are gone;
  - `production-export` saves each function's configuration into git.
- **Web UIs** are reached through an HTTPS ingress under their own hostnames
  (`https://<instance>.<platform>.<domain>`).

### A.1 Guideline (each item MUST unless marked SHOULD; N/A only with a reason)

**G1 Configuration**
- Every setting comes from environment variables, optionally also from one JSON config
  file.
- Precedence: env, then file, then defaults. Unknown env variables are ignored; invalid
  values exit with code **78** and a clear message.
- Every setting is documented in one table (README or SPECIFICATION).
- State the app writes itself lives only under one configurable directory (default
  `/config`), which survives restarts when mounted.
- Secrets are never logged.

**G2 MXL domains**
- `MXL_DOMAIN_SCAN_PATH` (default `/Volumes/mxl`) is the parent of the domain
  directories. Input domains are discovered below it, mirrors included.
- `MXL_OUTPUT_DOMAIN_DIR` and `MXL_OUTPUT_DOMAIN_ID` set the function's own output
  domain. Create it if missing.
- If a `domain_def.json` already exists with a different id, log an error and don't
  overwrite it.
- A new `domain_def.json` carries `id`, `label`, `description` and `tags`.
  BCP-007-03's schema requires all four, and readers skip domains without a
  description.
- Never write into another function's domain; never rewrite your domain files on every
  start.
- `history_duration` is configurable (`MXL_HISTORY_DURATION_NS`, default 200 ms).

**G3 NMOS identity**
- `NMOS_SEED` derives every id deterministically (UUIDv5): node, device, sources,
  flows, senders, receivers, and the default output domain id. The same seed gives the
  same ids after a restart.
- `NMOS_LABEL` sets the node label (and the device label prefix).
- `NMOS_TAGS` (a JSON object of tag name → array of strings, e.g.
  `{"urn:x-example:production":["sport-sa"],"urn:x-example:function":["mix1"]}`) is added to the
  node and device. BCP-002 group hints stay.

**G4 Registry, no DNS-SD**
- `NMOS_REGISTRY_ADDRESS`, `NMOS_REGISTRY_PORT`; `NMOS_QUERY_ADDRESS` (default: the
  registry address) and `NMOS_QUERY_PORT` (default: registration port + 1).
- `NMOS_DNS_SD` defaults to `false` and then switches off **both** DNS-SD browsing and
  mDNS self-advertisement. No Avahi or D-Bus requirement when it is off.

**G5 Announce IP addresses, never names**
- Everything announced to other systems is an **IP address literal**: the IS-04 node
  `href`, `api.endpoints[].host`, IS-05 control hrefs, transport parameters, ICE
  candidates, and any address the UI offers for copying.
- The address comes from **`NMOS_HOST_ADDRESS`**: the pod IP on the pod network, the
  node IP on the host network. When unset, the first non-loopback IPv4 of the container.
- Never announce a hostname, pod name, Service DNS name, `0.0.0.0` or `127.0.0.1`.
- Exempt: browser URLs that must carry TLS names (e.g. `PREVIEW_WHEP_URL`).

**G6 Ports**
- Every listening port is configurable by env: web, NMOS (and its WebSocket at
  `NMOS_PORT`+1 if used), media and UI ports, including child processes you configure
  (e.g. the built-in MediaMTX).
- No hard-coded ports. Two instances on one host with distinct ports must work.
- If a port (TCP or UDP) cannot be bound, exit with code **75**.

**G7 Health and metrics**
- `/livez`: the process is alive. It never depends on the registry.
- `/readyz`: 200 only when the function serves and, with a registry configured, its
  node is registered (checked via the Query API or the registration response).
- `/metrics`: Prometheus text on the web port, prefix `mxl_audio_mixer_`.

**G8 Clean shutdown**
- On SIGTERM, within `SHUTDOWN_TIMEOUT_S` (default 10):
  1. stop media (fade outputs) and release the MXL writers and readers;
  2. deregister from the registry (an explicit DELETE of every resource, the node last);
  3. with `MXL_CLEANUP_ON_EXIT=true`, remove the function's own output domain directory
     (only its own; mirrors belong to the fabrics agent);
  4. exit with code **143**.
- Child processes (e.g. the built-in MediaMTX) and long jobs are stopped, not waited
  for.
- `MXL_CLEANUP_ON_EXIT` may default to false; the platform sets it to true.

**G9 IS-05 (BCP-007-03)**
- Senders report their active `mxl_domain_id` and `mxl_flow_id`.
- Receivers accept a staged PATCH with `sender_id`, `master_enable` and
  `transport_params`, with `activate_immediate` (and scheduled activations, TAI).
- `master_enable: false` stops reading cleanly.
- SHOULD (here: MUST): the active connections survive a restart (kept in the state
  directory).

**G10 Config export and import**
- `GET /api/v1/config/export` returns one JSON document (the show file).
- `POST /api/v1/config/import` accepts that document and restores the function's
  configuration.
- Secrets are left out unless explicitly requested; document it.

**G11 Image and CI**
- GitHub Actions builds, tests and pushes `ghcr.io/leeo86/mxl-audio-mixer` (a public
  package):
  - on `main`: `git-<sha7>` and `nightly-dev`;
  - on a `vX.Y.Z` tag: `X.Y.Z`, `X.Y`, `X`.
- The runtime image runs as uid 1000.
- OCI labels: `org.opencontainers.image.source`, `.revision`, `.licenses`, **`.version`
  = the release version** (set it explicitly: otherwise the base image's label, e.g.
  Ubuntu's `24.04`, shows through), and `io.dmf.mxl.revision` = the MXL commit.
- Tags are never moved; example manifests reference only tags that exist.

**G12 Kubernetes example**
- `deploy/` shows the conventions above:
  - the pod network;
  - the standard env names;
  - probes on `/livez` and `/readyz`;
  - `terminationGracePeriodSeconds` > `SHUTDOWN_TIMEOUT_S`;
  - the MXL root hostPath and a writable `/config` volume;
  - no `hostIPC`, and capabilities only as needed.

**G13 Documentation**
- README: settings table, ports, exit codes (0, 75, 78, 143), the API list, how to run
  on the platform.
- `CHANGELOG.md`; `SPECIFICATION.md` matches the code; `IMPLEMENTATION_PLAN.md` records
  decisions and deviations.

**G14 Tests**
- Unit tests for configuration parsing and the new behaviour.
- An integration test: start → ready → SIGTERM → node deregistered and own domain
  removed (with `MXL_CLEANUP_ON_EXIT=true`).
- CI is green.

## Appendix B — Platform conventions added after v1.0.0

### B.1 Preview contract (browser audio and video previews)

The platform runs **one shared MediaMTX** per platform. Functions publish their previews
to it; browsers play them by WHEP (or HLS). Every function MUST also work **without** it.

| Setting | Meaning |
| --- | --- |
| `PREVIEW_PUBLISH_URL` | RTSP base URL of the shared MediaMTX (e.g. `rtsp://mediamtx.<ns>.svc:8554`). **Unset:** start a built-in MediaMTX as a supervised child process (restart with backoff), on the own port block (§14 G6), and publish there. |
| `PREVIEW_PATH_PREFIX` | Path prefix of every stream (default `mxl-audio-mixer`; the platform sets `<production>/<instance>`). Streams are `<prefix>/<stream>`. |
| `PREVIEW_WHEP_URL`, `PREVIEW_HLS_URL` | Public base URLs the browser uses. The UI builds `<base>/<prefix>/<stream>/whep` (WHIP: `/whip`). Unset with the built-in MediaMTX: derived from the page's host and the own ports. |
| `MEDIAMTX_API_URL` | Optional: the MediaMTX API, for reader counts (encode only what someone listens to). |

- Publishing: RTSP, with the publisher reopening by itself when MediaMTX goes away and
  comes back (every ~2 s). The UI's WHEP client reconnects by itself too (the sibling
  functions reconnect in about 2 s).
- The mode (`own`/`shared`), the publish state per stream and the child's restarts are
  shown in the status API, the UI and the metrics.
- Reference implementations: `mxl-webrtc-monitor` 1.3.x (`src/ops/mediamtx.cpp`, the
  supervisor) and `mxl-replay` 1.4.x.

### B.2 Widgets for operator screens

The platform's production designer embeds widgets of several functions on one operator
screen, as iframes.
- **`GET /widgets`** returns a JSON array of
  `{id, title, params (a JSON schema of the query), min_size {w, h}, version}`.
  - GET only (405 with `Allow: GET` otherwise).
  - CORS only for origins listed in `WIDGET_FRAME_ANCESTORS`, with `Vary: Origin`. The
    API's own CORS headers must not be added on top: two `Access-Control-Allow-Origin`
    values make browsers refuse it.
- **`GET /widget/<id>?<params>[&theme=dark|light|transparent]`** returns the widget
  page.
  - No app chrome; the API is same-origin.
  - The header `Content-Security-Policy: frame-ancestors <WIDGET_FRAME_ANCESTORS>`, and
    **no** `X-Frame-Options`.
  - A bad parameter → 400; an unknown id → 404.
- **`WIDGET_FRAME_ANCESTORS`** (default `'self'`): a CSP source list (e.g.
  `'self' https://designer.<domain>`). An invalid value exits with 78.
- **Messages:** the page posts `{type: "widget-ready"}` and
  `{type: "widget-size", w, h}` to its parent window.
- Reference implementations: `mxl-color-corrector` 1.1.x, `mxl-srt-gateway` 1.4.x,
  `mxl-test-player` 1.2.x, `mxl-webrtc-monitor` 1.3.x, `mxl-replay` 1.4.x.

### B.3 Release notes for the platform

Platform operators pin each release by digest. Every GitHub release states:
- the image tag and digest;
- every new or changed setting (with defaults), port and volume;
- widget ids and params;
- exit codes if changed;
- anything the platform must set;
- what was tested on hardware.

Changes to names, labels, ports or volumes are announced before the release, not after.
