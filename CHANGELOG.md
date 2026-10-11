# Changelog

## Unreleased

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
- Peak and RMS meters per strip and bus.
- Metrics `plan_swaps_total`, `plan_compile_seconds`, `signals`, `nodes{type}`,
  `output_commit_lag_ms{output}`.
- `mxl-mixer-probe`, a test reader.

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
