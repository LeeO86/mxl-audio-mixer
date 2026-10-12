# Changelog

## Unreleased

### Licence

- The mixer is licensed under the GNU General Public License, version 3 or later
  (`GPL-3.0-or-later`), instead of MIT, so that it and every fork stay open
  source. No release was made under MIT.

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
