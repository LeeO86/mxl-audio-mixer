# Changelog

## 1.2.0 (in progress)

Milestone 8 (§19: the logic editor).

### Milestone 8 (part 1): logic graphs

- Logic graphs in the show (§11): blocks (`const`, `button`, a signal's mute,
  fader, voice and level, tally on air, a timer, `and`, `or`, `not`, `compare`,
  `edge`, `latch`, `set-mute`, `set-fader`) wired port to port. A loop is allowed
  only through an `edge`, a `latch` or a `timer`. A timer stays on for its
  duration after its input rises (rounded up to the 10 ms tick); a new rise
  starts it again. Evaluated at 100 Hz; actions run when their input
  changes; `dry_run` computes without acting. `/api/v1/logic`. The Logic tab
  draws the cables and lists the same graph under them.

## 1.1.0 (in progress, after 1.0.0)

Milestone 7 (§19: VAD, auto-level, auto-mix, ducking).

### Milestone 7 (part 1): voice detection, auto-mix, ducking

- Voice activity detection (§8.1) on the control side: per analysed signal
  (before its fader) speech active, probability, speech level, onsets and
  offsets; band energy over the noise floor, voicing, modulation and tone
  rejection on 10 ms frames, threshold with hysteresis and hold. `/api/v1/vad`,
  `vad_active{signal}`.
- Gain-sharing auto-mixer (§8.3, after Dugan): NOM attenuation, weights, bypass,
  last-mic-hold, VAD weighting; muted members leave the sharing.
  `/api/v1/automixes`.
- Ducking by speech (§8.4): targets down by a depth while a key speaks, with
  attack, hold and release; also speech priority. `/api/v1/ducks`.
- An auto gain after the fader of auto-mix members and duck targets; the VAD
  state and the auto gains in the meters message; the console's Auto panel, a
  voice dot and the auto gain on strips.
- `mxl-mixer-tone --wav` (`--gap-s`, `--offset-s`): a recording instead of the
  tones.
- After the maintainers' answers and Com-IS-Assist's measurements: the auto gain
  is a chain node (`auto`), before the fader by default; snapshots hold the auto
  setup (section and class `auto`, recall-safe auto-mixes and ducks); the VAD
  and duck holds are 800 ms; a gate on the momentary loudness (`gate_lufs`,
  −32 LUFS) against crosstalk replaces `min_level_dbfs`.
- `docs/release-notes/1.1.0-checklist.md`: tests on recordings of productions.
- Silero VAD (ONNX Runtime, the model vendored, MIT) is the default detector;
  the feature detector stays, per signal (`detector`) and as the fallback.

### Milestone 7 (part 2): auto-level and ratio ducks

- Com-IS-Assist's control core (Andreas Weiss, MIT) copied into
  `src/auto/comis/` with its notice: the four-state machine, the slow, mid and
  fast stages, the automix engine, their tests.
- The VAD taps carry every channel (records in one ring, `vad_drops_total`);
  BS.1770 loudness per tapped signal on the control side (momentary,
  short-term, a fast 50 ms reading, the channel weights); the VAD's gate uses
  it, and its state shows `short_term_lufs`.
- Auto-level (§8.2): `/api/v1/autolevels/<signal>`: a target (−23 LUFS
  short-term), a speech offset (+2 LU while the VAD speaks), a range (±12 dB),
  attack and release (1 s, 3 s), a freeze below −50 LUFS.
- Ratio ducks (`mode: "ratio"`): Com-IS-Assist's control, the keys held 5 LU
  above the targets (13 LU over-voice), interview passthrough when only the
  targets speak, at most 24 dB, a speed. Channel depth groups for both modes
  (front, centre, LFE, surround) and per-channel auto gains.
- Auto-levels in snapshots (the `auto` section, recall-safe); the meters
  message carries the auto-levels' and ratio ducks' states; the Auto panel
  edits both; the demo ducks its music to a ratio and auto-levels its voices.

## 1.0.0 (2026-10-13)

The first release: milestones 0–6 (§19). Release notes:
`docs/release-notes/1.0.0.md`.

### After the hardware re-run

- The mixer keeps its other threads off the DSP thread's core (its CPU and that
  CPU's SMT siblings) and says in `dsp.scheduling` whether the core is shared
  (a warning when a sibling is outside its CPUs); the platform must give the
  DSP thread a whole physical core (`full-pcpus-only`, or SMT off).
- The meter thread no longer allocates per block.
- `ci/amwa.sh`: the node's ports from `AMWA_NODE_PORT` and `AMWA_WEB_PORT`;
  the load test prints the DSP thread's core.

### After the first hardware round

- The DSP thread's load in the §16 test: 0.854 on the maintainers' Xeon Gold
  6136, 0.149 now on the development VM (0.378 before):
  - the meters (peak, true peak, PPM, energy, correlation) on a meter thread of
    their own, from blocks the DSP thread hands over (`meter_drops_total`);
  - true peak four phases at a time (3.6× faster), skipped where it cannot
    exceed the peak already held;
  - the dB conversions without libm (`log2`/`exp2` by bits and series, error
    below 1e-8 dB), the compressor and de-esser in passes over the block that
    vectorise; biquad cascades sample by sample through all sections (the same
    arithmetic); peak and energy sums that vectorise.
- `tests/hw/release/run.sh`: the `amwa` row as text (no `|` in the table), the
  node's name from `NODE_NAME` (in a container); `ci/amwa.sh`: its
  `/dev/shm/mixer-amwa.*` folders removed, the registry's ports from
  `AMWA_REGISTRY_PORT` (and a clear error when they are taken).

### Before the release

- The routing graph (§5): a Graph tab with inputs, strips, buses, monitors and
  outputs; sends drawn, edited and deleted in it (Vue Flow, MIT).
- Oscillator: white and pink noise (level as RMS, independent channels) and a
  logarithmic sweep (`sweep_to_hz`, `sweep_s`).

### Milestone 6 (part 2): N-1 and group operations

- Mix-minus buses (`minus: {of, without}`): every send into another bus except
  some, following it without a plan swap.
- Operations (§5): group (a subgroup bus taking over the strips' post-fader
  sends), N-1 per participant or of a selection, mix-minus, link (mono strips into
  a stereo or surround strip) and unlink. `/api/v1/operations/…`; the console's
  Operations panel; mix-minus in the bus settings.
- `mxl-mixer-probe --correlate`.

### Milestone 6 (part 1): VCAs and mute groups

- VCA (DCA) groups (§9): nested, the members' faders offset by every VCA above
  them, VCA mute without touching the members' mutes, PFL/AFL of all members,
  spill on the console, in snapshots (section `vcas`), recall-safe; never a plan
  swap. `/api/v1/vcas`.
- Mute groups. A mute-safe (isolated) strip or bus is muted neither by a mute
  group nor by a VCA. `/api/v1/mute-groups`.
- Faders (strips, buses, VCAs) go to +15 dB; a fader and its VCAs add up to at
  most +15 dB. VCAs in layout pages and the `fader-bank` widget. Deleting a
  signal takes it out of layout pages.
- UI: VCA strips beside the buses, the VCAs and groups panel, mute group buttons,
  VCA chips on the strips; the `vca` widget.

### Licence

- MIT, as before. On 2026-10-12 `main` was GPL-3.0-or-later for a few hours; that
  was taken back the same day, before any release.

### Milestone 5 (part 3): talkback

- Talkback (§10.2): mics on MXL input channels and browser microphones (WHIP to
  MediaMTX, read back over RTSP, Opus decoded with loss concealment, a jitter
  buffer); destinations on buses, mixing or interrupting (IFB) with per-mic
  priority; the control room's talkback dim (`talkback_dim_db`); latching or
  momentary talk; listen-back of a destination's return; strips patched to a
  talkback mic (`{"talkback": "<mic>"}`).
- `/api/v1/talkback…`, `/api/v1/listen-back`; the talkback panel in the Control
  room tab; the `talkback` widget.
- `mxl-mixer-probe --onsets` (burst onsets in the TAI second, for latency).

### Milestone 5 (part 2): monitors in the browser

- The preview contract (Appendix B.1): monitors encoded as Opus (libopus) and
  published over RTSP (the mixer's own client) to MediaMTX, played by WHEP; a
  built-in MediaMTX as a supervised child process (`MEDIAMTX_*`), or a shared one
  (`PREVIEW_PUBLISH_URL`, `PREVIEW_WHEP_URL`, `MEDIAMTX_API_URL`). Streams run only
  while someone listens. The image has MediaMTX v1.20.1.
- Personal monitors (`MIXER_MAX_WEB_MONITORS`), `/api/v1/preview`,
  `/api/v1/personal`; Listen on the monitor cards and "My headphones" in the
  Control room tab; `preview_*` metrics.
- The `second-pulse` oscillator waveform; the latency measured in headless Chrome
  (medians 79–99 ms to the audio output).
- Fixed: a toggle pressed twice quickly could stay on when a mix pushed before the
  first press arrived in between (`x-mix-revision`); at most one change per path
  in flight.

### Milestone 5 (part 1): control room

- Monitors in the show (`control_room`): source (strip, bus or input), level, dim,
  cut, mono/difference/swap/left/right checks, room alignment (trim, delay, chain),
  SPL presets, published as MXL flows with NMOS senders; bass management (LR4
  crossover to the LFE) switched on and off per monitor. Dim and the programme
  under solo default to −21 dB.
- Solo: PFL and AFL on every strip and bus, additive or interlock, PFL trim, the
  programme under solo, solo-in-place with solo-safe signals.
- The `oscillator` node (sine, EBU R 49 stereo ident).
- `/api/v1/control-room`, `/api/v1/monitors`, `/api/v1/solo`; the Control room tab,
  PFL/AFL on the strips, the `monitor` widget.
- CI runs the integration tests on release binaries.

### Widgets and tally

- Widgets for operator screens (Appendix B.2): `GET /widgets`, `/widget/strip`,
  `/widget/fader-bank`, `/widget/loudness`; `WIDGET_FRAME_ANCESTORS`.
- Tally: TSL UMD 5.0 receiver on UDP and TCP (`TSL_ENABLE`, `TSL_UDP_PORT`,
  `TSL_TCP_PORT`, `TSL_SCREEN`, `TSL_MAP`), `GET /api/v1/tally`,
  `tally_packets_total`; tally lamps on the console.
- Audio-follows-video per strip (`tally_index`, `afv`): open while on air, at its
  depth otherwise, with fade times.

### Milestone 4: console UI, processing editor, routing matrix, snapshots, show files

- Web UI (Vue 3, one page embedded in the binary, the family look): console with
  banks and layouts, processing chain editor (palette, drag, curves with handles),
  routing matrix and published outputs, snapshots, layouts and show history,
  metering (bridge on dBFS, true peak and PPM scales, loudness, spectrum,
  goniometer), NMOS, status, settings; light, dark or system theme.
- `/api/v1/events` WebSocket: status, the mix on every change, meters at 25 Hz with
  per-client subscriptions, the analyser.
- Snapshots (`/api/v1/snapshots`): recall filtered by signal, section and parameter
  class, recall-safe signals, crossfade. Layouts (`/api/v1/layouts`), strip and bus
  colours. Show history (`/api/v1/shows/history`): automatic and named versions,
  restore; an import keeps the show before it.
- Meters: PPM (IEC 60268-10 Type I and II) and phase correlation; the analyser
  (`/api/v1/analysis`): 1/6-octave spectrum and goniometer of any tap.
- Loudness: blocks the DSP thread missed count as silence instead of restarting
  momentary and short-term.
- `scripts/demo.sh`, `tests/integration/show.sh`, `tests/integration/ui.sh`
  (headless Chrome), `tests/perf/section16.sh`.

### EBU test set

- Loudness checked on the EBU loudness test set v5 in CI: Tech 3341 cases 1–23,
  Tech 3342 cases 1–6. Momentary and short-term loudness now update every 10 ms.

### Milestone 3: EQ, filters, dynamics, delay, delay compensation, loudness

- Processing chains on strips and buses: EQ (8 bands), Butterworth filters
  (12–48 dB/oct), dynamic EQ, compressor (soft knee, peak/RMS, sidechain filter,
  key), look-ahead limiter (optional true peak), expander, gate, de-esser
  (split-band, wide-band, listen), ducker, multiband (3–5 bands), delay; bypass;
  per-node gain reduction and in/out peaks. API under `/api/v1/{strips,buses}/<id>/chain`.
- Taps pre/post-EQ, pre/post-dynamics and after any node; keys from other signals.
- Delay compensation per bus (`compensate`), latency per output; loops through
  block-delay sends.
- True peak (4× oversampling) on every meter; loudness (BS.1770-4, EBU R 128,
  Tech 3341/3342) per signal with start/stop/reset; metrics `loudness_lufs` and
  `true_peak_dbtp`.
- After a missed deadline the skipped output blocks get silence instead of stale
  ring data. `mxl-mixer-tone` catches up instead of skipping.

### Milestone 2: plan compiler and DSP core

- The mix: strips with input patch, trim, polarity, fader, mute and panner
  (pan law, balance, width); buses; a send matrix with taps, gain, mute and phase;
  width mapping between mono, stereo, 3.0, 5.1, 7.1, 5.1.4 and 7.1.4 (BS.775
  down-mix). Kept in `mix.json` and in the show file; the mix API under
  `/api/v1/{mix,strips,buses,sends,outputs,meters}`.
- The plan compiler: mix → execution plan; buses in dependency order, loops
  rejected. Parameter changes ramp (10 ms default); a rewiring fades the affected
  paths out and in around the swap; unchanged nodes keep their state.
- Published buses: MXL flows in the own domain with stable flow ids (UUIDv5 of
  `NMOS_SEED` and the bus id), commit staging (`MIXER_OUTPUT_COMMIT_SAMPLES`, per
  output overridable), fades on publish and on SIGTERM; NMOS Source, Flow and
  Sender per output.
- Peak, peak hold (cleared through the API) and RMS meters per strip and bus.
- Metrics `plan_swaps_total`, `plan_compile_seconds`, `signals`, `nodes{type}`,
  `output_commit_lag_ms{output}`.
- `mxl-mixer-probe`, a test reader.
- `mxl-nmos` v0.1.1: scheduled IS-05 activations fire at their time.

### Milestone 1: skeleton

- Settings from the environment, `MIXER_CONFIG_FILE` and defaults; invalid values
  exit 78; `GET /api/v1/config` with every value's origin.
- `/livez`, `/readyz` (with details, including `timing_trusted`), `/metrics`
  (`mxl_audio_mixer_`), `/api/v1/status`, export and import of the show file.
- The NMOS node (`mxl-nmos` v0.1.0) with `MIXER_INPUT_FLOWS` MXL audio receivers;
  the active connections survive a restart.
- The DSP thread on the TAI block clock reads every routed input at
  t − (`MIXER_READ_OFFSET` + extra delay); FTZ/DAZ, timer slack 1 ns, CPU pinning,
  optional SCHED_FIFO ≤ 49; pools sized from the cgroup CPU limit.
- Inputs wait for flows that do not exist yet, re-open re-created flows, report
  stalls, fade over gaps, and measure the arrival lag; the extra delay is `auto` by
  default.
- The own output domain (G2) and, on SIGTERM, deregistration and optional removal
  (G8), exit 143.
- `mxl-mixer-tone`, a test writer. Image `ghcr.io/leeo86/mxl-audio-mixer`, a
  Kubernetes example and a Grafana dashboard.

### Milestone 0

- Spikes: an `mxl-nmos` node on real MXL flows, the TAI block clock with measured
  jitter (`spikes/m0`, `IMPLEMENTATION_PLAN.md`).
