# Implementation plan and record

Decisions, deviations and results against `SPECIFICATION.md` (Draft v0.3). Kept
current so that work can resume after an interruption.

## Status (2026-10-11)

| Step | State |
| --- | --- |
| M0a `mxl-nmos` extracted and tested | done: PRs 1–3 merged, **v0.1.0 released** (`88e04c5`); **v0.1.1** (`f949a69`, PRs 5–7: DNS names, scheduled activations on time) |
| M0a Strom switched to the crate | patches against Strom `c2e27a6` in `LeeO86/mxl-nmos` `contrib/strom-switch/` (PR 4); the maintainer applies them in Strom (this environment cannot push there) |
| M0b spike: NMOS audio receiver + sender on real flows, AMWA | done (below) |
| M0c spike: MXL float32 read/write on the TAI block clock, jitter | done on the dev VM (below); real-time run: `tests/hw/m0-block-clock` (maintainers) |
| M1 skeleton, config, ops, metrics, CI, image, MXL I/O | done, merged (PRs 1 and 2) |
| M2 plan compiler and DSP core, buses, sends, outputs, meters, input patch | done, in review (branch `cursor/m2-plan-dsp-core-4d44`, PR 3) |

## Resume

- `mxl-nmos` v0.1.1 is released; the mixer depends on the tag.
- Strom: the four patches are in `mxl-nmos` `contrib/strom-switch/` (README: how to apply).
- This repo: `main` has M0 and M1; M2 is on `cursor/m2-plan-dsp-core-4d44`.
- Environment: `docs/development.md`.

## Decisions

| # | Decision | Reason |
| --- | --- | --- |
| D1 | MXL through the **official Rust bindings** (`dmf-mxl/mxl` `rust/mxl`, `rust/mxl-sys`) at the pin `218ddaa`, feature `mxl-not-built`; libmxl built separately (`scripts/build-mxl.sh`, FABRICS_OFI off) and loaded at run time. | §3: official bindings when available at the pin. Their own build script would build libmxl without `-DMXL_ENABLE_FABRICS_OFI=OFF`. |
| D2 | NMOS through `mxl-nmos` (git rev now, release tag once it exists). | §3.1. |
| D3 | The block thread sleeps with `clock_nanosleep(CLOCK_TAI, TIMER_ABSTIME)` to the TAI time of the block's last sample + 1, then writes block n and reads block n − R. | §4.4. One clock for MXL indices and deadlines. |
| D4 | The block thread sets timer slack to 1 ns (`PR_SET_TIMERSLACK`), FTZ/DAZ, optional SCHED_FIFO (`MIXER_RT_PRIORITY`, capped at 49) and CPU pinning from the affinity mask. | §2, §18. Timer slack: measured below (p50 70 → 19 µs). |
| D5 | Readers are opened outside the block thread and handed over through a channel; an activation is accepted at once and the receiver is `waiting` until its flow exists. | §2 real-time rule, §3.1. |
| D6 | Licences: the `mxl` crates are Apache-2.0, `mxl-nmos` MIT OR Apache-2.0; both can be used in this MIT project (the binary's notices then include Apache-2.0). | §3 "check every dependency's licence". |

## Deviations

| # | Spec | Deviation | Reason |
| --- | --- | --- | --- |
| X1 | §4.3 `MIXER_OUTPUT_COMMIT_SAMPLES` | The spike creates writers without options (libmxl's default hint, 480 samples). From milestone 1 the mixer creates writers through `mxl-sys` directly (a small unsafe wrapper) so the commit and sync hints can be set; back to the safe API when a v1.1 release has the fix. | Bug in the bindings at the pin: `MxlInstance::create_flow_writer` frees the options string before libmxl reads it (use after free; libmxl logged "Invalid JSON options"). Fixed upstream on `main` (`aed4cac6`), not on `release/v1.1`. Approach approved (Q1). |

## M0 results

### mxl-nmos (LeeO86/mxl-nmos PR 3)

- 47 tests (`cargo test`), nmos-cpp registry integration test, CI green on Linux,
  macOS and Windows.
- AMWA nmos-testing `9001851` against the crate's example node: 0 Fail, 0 Warning.
  DNS-SD off: IS-04-01 Pass 39, IS-05-01 Pass 53, IS-05-02 Pass 55, BCP-007-03-01
  Pass 55 + Manual 1 (test_15). DNS-SD on: IS-04-01 Pass 54 (mDNS discovery,
  priorities, failover, timeouts, stale-registration handling), the others as above.
  Every non-Pass result is listed with its reason in `ci/amwa-expected-*.json`.

### Strom with the crate (local)

- Strom's NMOS tests: 12 of 12 pass (the MAC and registry-URL tests moved to the crate
  with their code); Strom's whole suite: 885 passed, 0 failed, 4 ignored (as on `main`);
  clippy `-D warnings` clean with `efp,mxl,nvidia`.
- AMWA BCP-007-03-01 against Strom: Pass 55, Manual 1, 0 Fail (same as PR #11's run).
- All IS-04 resources and IS-05 documents of the old and new node, same seed and flows,
  compared field by field without `version`: identical except the two intended changes
  (Device `controls` lists the Connection API; Node `href` is the API base URL).

### Spike 1: NMOS audio receiver and sender on real MXL flows

`spikes/m0`: one Device; Source + Flow + Sender for a real 2-channel `audio/float32`
flow written into the spike's own domain; one Receiver that reads the activated flow.

- AMWA against the spike: IS-04-01 Pass 39, IS-05-01 Pass 53, IS-05-02 Pass 55,
  BCP-007-03-01 Pass 55 + Manual 1; 0 Fail, 0 Warning; node and devices gone from the
  registry after SIGTERM.
- IS-05 loopback (receiver connected to the own sender): 5 756 736 of 5 760 096
  samples read in 120 s (the rest before the activation), **0 mismatched samples**,
  0 read or write errors.
- Activation before the flow exists: answered 200 at once, receiver `waiting`; a second
  process created the flow 5 s later, the receiver was `running` 18 ms after that and
  read 1 439 328 samples from the other process with 0 mismatches.
- When that writer exited, its flow was deleted and the reader failed on every block
  (2 483 errors in the remaining 5 s), each failed read spending the 500 µs read
  timeout inside the block (block work p99 536 µs). Milestone 1 handles this
  (stall detection, re-open; §4.2).

### Spike 2: TAI block clock, 96-sample blocks (2 ms), dev VM

4 vCPUs shared with other work; SCHED_FIFO is refused in this sandbox even for root,
so these are SCHED_OTHER numbers. Wake jitter = wake-up time − block deadline on
`CLOCK_TAI`; block work = write + read + checks; arrival lag = TAI now − flow head.

| Run (release build) | Blocks | Wake jitter p50 / p99 / p99.9 / p99.99 / max (µs) | Deadline misses | Work p50 / p99 / max (µs) |
| --- | --- | --- | --- | --- |
| default timer slack | 60 001 | 70 / 251 / 377 / 536 / 1 691 | 0 | 11 / 14 / 35 |
| timer slack 1 ns, pinned CPU | 60 000 | 19 / 190 / 319 / 1 188 / 1 417 | 0 | 11 / 14 / 35 |
| same, `tests/hw` script dry run | 30 000 | 19 / 179 / 322 / 1 220 / 4 679 | 3 | 11 / 14 / 66 |

All three runs: 0 mismatched samples and 0 read errors; the 2 ms read offset absorbed
the late wake-ups. The 4.7 ms outlier on the shared VM is what §18 expects without
SCHED_FIFO on exclusive CPUs; the real-time run is `tests/hw/m0-block-clock`.

### Findings for later milestones

- **MXL indices are head indices**: `(index, count)` covers `[index − count, index)`
  for both writer and reader (`flow.h`). A first version wrote every block one block
  early; reading with the same mistake hid it, the arrival lag (2.08 ms instead of
  0.08 ms) showed it.
- Default timer slack (50 µs) is the floor of every SCHED_OTHER wake-up.
- A failed blocking read costs its whole timeout inside the block: the read timeout
  must stay small against the block period, and stalled inputs must stop being read
  (§4.2).
- The writer's flow is deleted when it releases it (§4.2 confirmed).
- The VM's kernel TAI offset was 0 (CLOCK_TAI = UTC); NMOS scheduled activations were
  37 s late until it was set. §4.4's start-up check is needed.

## M1: skeleton

### What exists

- `src/config.rs`: every setting in one table (README generated from it; a test
  fails when one is missing), env > `MIXER_CONFIG_FILE` > defaults, origins, exit 78.
- `src/engine/`: the DSP thread. Block n ends at TAI index (n+1)·B; it sleeps with
  `clock_nanosleep(CLOCK_TAI, TIMER_ABSTIME)`, reads each routed input's block ending
  at (n+1)·B − (read offset + extra delay) with a bounded wait of a quarter block,
  fades in after a gap and out over one (10 ms, the ramp of §2), and counts late and
  missing samples (start-up silence is not counted). After a deadline miss it goes on
  with the first block that has not ended. Lock-free SPSC rings (`rtrb`) carry
  commands in and readers (freed on the control side) and arrival lags out; counters
  are atomics. Buffers are allocated before the loop.
- `src/inputs.rs`: the control side on a 100 ms tick: routing from IS-05, `waiting`
  until the flow exists, re-open on a new inode or after a stall, `input_stalled` /
  `input_resumed` logged once per change, the arrival-lag window and the automatic
  delay.
- `src/mxlio/`: libmxl through the dynamic linker, domains (G2), and the
  `mxl-sys` writer wrapper that passes the commit/sync hints (X1).
- `src/nmos.rs`, `src/ops.rs`, `src/state.rs`, `src/main.rs`: the node, the web
  port, the files under `MIXER_STATE_DIR`, start-up and the G8 sequence.
- `src/bin/mxl-mixer-tone.rs`: the test writer (also in the image).
- `docker/Dockerfile`, `.github/workflows/{ci,publish}.yml`, `deploy/k8s`,
  `deploy/grafana`.

### Decisions

| # | Decision | Reason |
| --- | --- | --- |
| D7 | No outputs are published in M1; the writer path (`mxlio::writer`) is exercised by `mxl-mixer-tone` in the tests. Senders come with the signals of M2. | §19 puts NMOS senders and the signal model in M2; a placeholder output would invent behaviour. |
| D8 | Inputs are numbered from 1 everywhere (`in-1`, "Input 1", `input="1"`, `/api/v1/inputs/1`). | One number for operators, NMOS and metrics. |
| D9 | Setting names follow the sibling repos: `HOST_ID`, `MIXER_CONFIG_FILE`, `MIXER_STATE_DIR` (default `/config`), `MIXER_LOG_LEVEL`, `MIXER_BLOCK_SAMPLES`, `MIXER_INPUT_STALL_S` (default 2 like `REPLAY_INPUT_STALL_S`); default seed `<HOST_ID>-mixer`, node label `HOST_ID`, Device "<label> Audio Mixer", output domain `<scan>/mixer-<8 hex>` (the `cc-<8 hex>` pattern). | Maintainer answer 2. |
| D10 | `MIXER_READ_OFFSET` keeps the spec's name, in ms (may be fractional). | §4.2 names it and gives "2 ms". |
| D11 | Automatic delay: extra = p99.9(lag) + margin − read offset, clamped to `MIXER_INPUT_DELAY_MAX_MS`; changed when it differs by more than the margin. Lags count only while the flow's head moves (a stopped writer is a stall). | §4.2. Without the last rule a stall drove the delay to its maximum (found in testing). |
| D12 | Readiness = the engine runs and, with a registry, the node is registered and listed by the Query API. `/readyz` answers JSON details; an untrusted TAI offset is reported there and in `timing_trusted` but does not block readiness. | G7, §4.4 ("start anyway"). |
| D13 | Import restores inputs and connections (through `restore`, so the activation callback runs); settings in the file are not applied. | G1: settings come from the environment and the config file. |

### Results (dev VM)

- `cargo test`: 14 unit tests; clippy `-D warnings`, rustfmt and shellcheck clean.
- `tests/integration/lifecycle.sh`: ready (registered, listed by the Query API),
  SIGTERM → exit 143, node 404 in the Query API, own domain removed and another domain
  kept; after a restart the connection is active again and the node id is the same.
- `tests/integration/inputs.sh`:
  - activation before the flow exists → `waiting`, → `running` once the writer starts;
  - 0 missing samples over 3 s while running;
  - writer stopped (SIGSTOP) → `stalled` after 2 s and staying stalled, → `running` on SIGCONT;
  - writer restarted (flow deleted and re-created) → re-opened by itself, 0 missing
    samples afterwards;
  - writer 30 ms late → arrival lag p99.9 32.25 ms, effective delay settled at
    33.08 ms, 0 missing samples over the next 5 s;
  - `master_enable: false` → `unrouted`.
- AMWA against the mixer (DNS-SD off, nmos-cpp registry): 0 Fail, 0 Warning.
  IS-04-01 Pass 29, IS-05-01 Pass 33, IS-05-02 Pass 38, BCP-007-03-01 Pass 40 +
  Manual 1. Every sender test is Could Not Test until M2 publishes outputs; all of
  them are listed in `ci/amwa-expected-off.json`.

### Answers (maintainer, 2026-10-11)

- Automatic delay: `MIXER_INPUT_DELAY_MAX_MS` default 150 ms, margin/hysteresis
  1 ms, lag window 10 s — accepted.
- Defaults "Input N", "<label> Audio Mixer", seed `<HOST_ID>-mixer`, output domain
  `<scan>/mixer-<8 hex>` — accepted.
- `NMOS_REGISTRY_ADDRESS` / `NMOS_QUERY_ADDRESS` accept DNS names (a Service name on
  the platform); announced addresses stay IPv4 literals (G5). The same change goes
  into `mxl-nmos` `Settings::apply_env` (0.1.1).
- M0 and M1 merged with the maintainer's approval.

## M2: plan compiler and DSP core

### What exists

- `src/mix/`: the mix model (strips, buses, sends, outputs, mix settings) with its
  validation, the channel formats and width mapping (`format.rs`), and the plan
  compiler (`compile.rs`).
- `src/engine/plan.rs`: plans and their executor, a pure function of the inputs
  (unit-tested without MXL): patch, gain, panner matrix, clear, send (with a width
  mapping), meter and output steps over numbered block buffers; nodes with
  smoothed values; parameter messages by stable node id.
- `src/engine/dsp.rs`: the DSP thread swaps plans at a block boundary, applies
  parameter messages, and writes the output slots (staging for the commit size,
  fade in on publish, fade out on SIGTERM).
- `src/control.rs`: stage → (fade wait) → swap; `src/outputs.rs`: MXL writers and
  NMOS senders; `src/meters.rs`; `src/api.rs`: the mix API.
- `src/bin/mxl-mixer-probe.rs`: a reader that checks a flow sample by sample against
  the `mxl-mixer-tone` signal.

### Decisions

| # | Decision | Reason |
| --- | --- | --- |
| D14 | The signals of M2 are strips and buses. Auxes, groups and N-1s are buses with their own sends; the N-1 and group operations come with M6, monitor paths with M5. Inputs reach the matrix through the input patch and strips, not as send sources. | §19 puts buses and the input patch in M2, N-1 and groups in M6. A raw input has no fixed format (its flow decides the channel count), a strip does. |
| D15 | Strip chain in M2: patch → trim and polarity → fader and mute → panner. Taps: `input`, `pre-fader`, `post-fader`, `post-pan`. The pre/post-EQ and pre/post-dynamics taps and "after any node" come with those nodes in M3. | §5 tap list, minus the nodes that do not exist yet. |
| D16 | Panner: a mono strip becomes stereo with the pan law (its post-pan tap is stereo). A stereo strip has balance and width (M/S; 0 mono, 1 unchanged, 2 double side). Wider formats have no panner. | §6 "pan/balance/width". The spec has no surround panner. |
| D17 | Channel order as SMPTE ST 2110-30 (L R C LFE, surrounds, heights). Width mapping by channel label: equal labels 1:1; missing ones fold down at −3 dB per step (ITU-R BS.775, LFE dropped): C → L + R, Ls → L (or Lss), Lss/Lrs → Ls, Ltf → L, Ltr → Lrs (or Ls), stereo → mono with the stereo-to-mono sum, mono → C (3.0 and surround) or L + R with the pan law (stereo); up-mixing leaves new channels silent (5.1 → 7.1 puts Ls on Lss). NMOS Source channels carry the VSF TR-03 symbols where TR-03 has one (`M1` for mono; heights have none). | §5 "up/down-mix presets for 5.1/7.1"; §4.1 formats incl. 3.0 and 5.1.4 (spec change of 2026-10-11). One rule set covers every pair of the seven formats. |
| D18 | Only buses are published (§4.3 lists bus, output, N-1, aux, group and monitor path; in M2 all of them are buses). An output is the bus post-fader; `format` maps it to another layout with D17. | §4.3. |
| D19 | Changes of the mix are compiled on the control side. When only values differ (same node signatures), the new values go to the DSP thread as ramped messages (`ramp_ms`, default 10 ms). Otherwise the nodes whose wiring changes or that disappear ramp to 0, the API waits 10 ms + 3 blocks, then the new plan is swapped in; nodes with the same signature carry their current values over, new ones ramp in from 0. A rewiring answers about 16 ms later (block 96). | §2 (state preserved, nothing clicks), §17 (swap without discontinuities). |
| D20 | Output writers live in DSP-thread slots, independent of the plan (writers stay open across swaps, §4.3). The commit size is staged in the slot: block n is committed when the staging holds `commit_samples`; after a deadline miss a partial staging is dropped. Publish fades in, SIGTERM fades out (10 ms) before the writers are released. | §4.3 commit size, G8 "fade outputs". |
| D21 | Meters in M2: sample peak (since the last 40 ms update), a peak hold (highest peak until cleared with `POST /api/v1/meters[/<id>]/clear`; kept across plan swaps) and RMS over 300 ms per strip and bus, post-fader, through `GET /api/v1/meters`. True peak, loudness and PPM ballistics come with M3, the WebSocket with the UI (M4). | §19 (true peak and loudness in M3); §7's 25 Hz update. |
| D22 | MXL flow id of an output = UUIDv5(`NMOS_SEED`, `output/<bus>/mxl-flow`); it differs from the IS-04 Flow id (allowed by BCP-007-03). NMOS keys `out-<bus>` for Source, Flow and Sender; labels = bus name; group hint `<name>:Audio`. | §4.3, G3. |
| D23 | IS-05 activations of a sender are accepted and reported; they do not stop or move the flow, which is written while the bus is published. | The flow belongs to the published signal; readers find it by its stable id. |
| D24 | −∞ is `"-inf"` in JSON (fader and send gains). | JSON has no infinity. |
| D25 | A `mix.json` that cannot be read or validated is moved to `mix.json.bad` and the mixer starts with an empty mix. | Never overwrite an operator's show by accident. |
| D26 | The first mix is empty: no strips, buses or outputs until they are created through the API or an import. | The spec has no default layout; inventing one would publish NMOS senders nobody asked for. |

### Deviations

None.

### Results (dev VM)

- `cargo test`: 38 unit tests, including:
  - pan laws and BS.775 coefficients;
  - feedback rejection;
  - a plan swap that keeps unchanged nodes without a discontinuity;
  - linear, bounded ramps;
  - summing with width mapping;
  - proptests for the mapping bounds, the constant-power law and ramps.
- Clippy `-D warnings`, rustfmt and shellcheck are clean.
- `tests/integration/mix.sh`, every check sample-exact (max error < 1e-5) with
  `mxl-mixer-probe`:
  - a mono strip centred into a stereo bus: L = R = tone × 0.7071;
  - 15 plan swaps (other buses and sends created and deleted) while reading: the
    output stayed sample-exact;
  - mute (ramped) plus a new send at −6.02 dB with phase: L = −0.5 × tone 1,
    R = −0.5 × tone 2;
  - pan hard left and a deleted send: L = tone 1, R = 0;
  - the bus published as mono: (L + R) × 0.7079;
  - meter peak −20.0 dBFS for a −20 dBFS tone;
  - the NMOS sender reports the output's `mxl_flow_id`;
  - SIGTERM deletes the flow;
  - after a restart the flow id is the same, the mix and the connection are
    restored, and a second mixer reading the output is `running` again with 0
    missing samples over 2 s;
  - unpublish removes the flow and the sender;
  - a bus loop is rejected with the loop in the message.
- `lifecycle.sh` and `inputs.sh` pass unchanged (writer 30 ms late: arrival lag
  p99.9 32.25 ms, effective delay 33.08 ms, 0 missing).
- AMWA with one published stereo bus, 0 Fail and 0 Warning:
  - IS-04-01: Pass 39;
  - IS-05-01: Pass 53;
  - IS-05-02: Pass 55;
  - BCP-007-03-01: Pass 55 + Manual 1.

  The M1 "no senders yet" entries are gone from `ci/amwa-expected-off.json`. Six
  IS-05-01 sender tests stay Could Not Test because the tool leaves MXL to
  BCP-007-03-01.
- Load (release build, SCHED_OTHER on the shared 4-vCPU VM, block 96):
  - mix: 64 mono strips (patched, panned), 16 stereo buses, 264 sends, 8 published
    outputs (80 meters);
  - over 60 s: mean block time 61 µs (`dsp_load_ratio` 0.031), 99.85 % of blocks
    ≤ 100 µs, max bucket ≤ 500 µs;
  - 2 wake-up deadline misses (the VM without real-time scheduling, as in M0);
  - 0 missing input samples, 0 output errors, commit lag 0.08 ms.

  The §16 target load (EQ, dynamics, VCAs) is measured from M3 on.

### Findings

- AMWA IS-05-01 test_28 ("relative activation later than expected") warned in the
  first `main` run after the M1 merge. It reads `/active` exactly 200 ms after the
  PATCH reply, and the crate's scheduled activations fired on tokio's 1 ms timer.
  Measured with a probe of the same timing: the mixer was late in 23 of 150 runs
  and the crate's example in 4 of 150. Fixed in `mxl-nmos` PR 6 (prepare 2 ms
  early, block on the system clock for the last part): 0 of 300 and 1 of 300.
  Released in `mxl-nmos` v0.1.1, which the mixer uses from M2 on.
- `NMOS_REGISTRY_ADDRESS` / `NMOS_QUERY_ADDRESS` DNS names in the crate:
  `mxl-nmos` PR 5. The mixer already accepts them in its own settings (M1).
- A mixer reading another mixer's output on the same TAI block grid sees an
  arrival lag of about one block (p50 2.08 ms with 96-sample blocks): both wake at
  the same deadline, and the reader finds the previous block. The automatic delay
  covers it (3.25 ms effective in the hw dry run).

### Answers (maintainer, 2026-10-11)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Defaults of a new strip, bus, send | Fader 0 dB, unmuted, pan centre, width 1; a send 0 dB from a strip's post-pan or a bus's post-fader. Accepted. |
| 2 | Trim range | ±42 dB (changed from the proposed ±24). |
| 3 | Stereo width range | 0–2. Accepted. |
| 4 | Default stereo-to-mono sum | −3 dB. Accepted. |
| 5 | Meters | Peak per 40 ms update and RMS over 300 ms, post-fader, accepted; plus a peak hold that is cleared manually (`POST /api/v1/meters[/<id>]/clear`). |
| 6 | Mono into 3.0 and surround | Into C (changed from the proposed front L/R pair). |
| 7 | Published outputs | At most 128. Accepted. |
| 8 | Com-IS-Assist (GPLv3) in this MIT repository | Copy it. The maintainer knows its developer: either that project changes its licence or this one does. The copy comes with milestone 7 (§8.2), with attribution and its licence noted; the licence question is a release blocker for the release that contains it until it is settled. |

## tests/hw scripts for the maintainers

| Script | What |
| --- | --- |
| `tests/hw/m0-block-clock/run.sh [minutes]` | Block clock at SCHED_FIFO 49 on an exclusive CPU: 0 deadline misses, max jitter < 2 ms, 0 read/write errors, 0 mismatched samples. |
| `tests/hw/m2-mirrored-output/run.sh source [commit]` (node A), `… check <sender> <domain> <flow> [s]` (node B) | §17 mirrored outputs through mxl-fabrics-agent at the default commit size and at 480: full sample rate, 0 gaps, sample error < 1e-5, 0 missing samples at a downstream mixer. |

## Answers (maintainer, 2026-10-11)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Writer options at the pin | Yes: an `mxl-sys` wrapper now, the upstream fix later (X1). |
| 2 | Names for block size, libmxl path, read timeout | Follow the sibling repos. They prefix function settings and put the unit in the name (`MV_INPUT_OFFSET_GRAINS`, `REPLAY_INPUT_STALL_S`), and none has a library path or read-timeout setting (they load libmxl from the system library path). So: `MIXER_BLOCK_SAMPLES` (default 96); no libmxl path setting (`libmxl.so.1` through the dynamic linker; the image installs it in the linker path); the read timeout is internal, bounded by the block period. |
| 3 | Tag `mxl-nmos` v0.1.0 | Merged PRs 1–3 and released v0.1.0 with the B.3 notes. |
| 4 | Strom PR | Delivered as patches in `mxl-nmos` `contrib/strom-switch/` (PR 4). |
