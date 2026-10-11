# Changelog

## Unreleased

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
