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
| M2 plan compiler and DSP core, buses, sends, outputs, meters, input patch | done, merged (PR 3) |
| M3 EQ, filters, dynamics, de-esser, delay; delay compensation; true peak and loudness | done, in review (branch `cursor/m3-eq-dynamics-loudness-4d44`, PR 4) |

## Resume

- `mxl-nmos` v0.1.1 is released; the mixer depends on the tag.
- Strom: the four patches are in `mxl-nmos` `contrib/strom-switch/` (README: how to apply).
- This repo: `main` has M0–M2; M3 is on `cursor/m3-eq-dynamics-loudness-4d44`.
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

## M3: EQ, filters, dynamics, delay; delay compensation; true peak and loudness

### What exists

- `src/dsp/`: biquads (RBJ cookbook, Butterworth cascades), dynamics building blocks
  (gain computers, ballistics, sliding minimum, look-ahead gain), the true-peak
  interpolator, BS.1770-4 K-weighting with the segment meter and the gating, loudness
  range.
- `src/mix/chain.rs`: the node types with validation, the smoothed parameter vector,
  structural signature and latency of each.
- `src/engine/proc.rs`: the processors as the DSP thread runs them (storage allocated
  on the control side), with per-block parameter ramps, the bypass crossfade, node
  meters and state handover.
- `src/mix/compile.rs`: all signals in dependency order; chains as process steps;
  named and after-node taps; keys; per-bus delay compensation; block-delay sends
  through a hold; true peak on every meter; loudness steps.
- `src/loudness.rs`: loudness meters per signal in DSP slots of their own, with
  start/stop/reset.
- API: chain nodes, loudness; meters with true peak and node meters.
- `tests/integration/processing.sh`.

### Decisions

| # | Decision | Reason |
| --- | --- | --- |
| D27 | A strip runs trim → chain → fader → panner; a bus runs sum → chain → fader. Trim, fader and panner stay fixed stages (D15); the chain holds the §6 nodes. | §2 "channel strips are processing graphs" with nodes added, removed and reordered at runtime; the console stages every strip has. |
| D28 | Every node parameter is required when a node is created; the engine invents no defaults. Ranges are validated (README table). | "Unknown values are never invented." The UI (M4) supplies its defaults. |
| D29 | Dynamics after Giannoulis, Massberg and Reiss (JAES 2012): feed-forward, quadratic soft knee, smooth branching ballistics on the gain in dB. Detection is linked: the largest level of all channels. `peak` is the instantaneous level; `rms` smooths the mean square with the attack (rising) and release (falling) times. Expanders and gates smooth the level instead, so that a sine's zero crossings do not close them. Attack and release are time constants (63 %). De-esser and ducker ballistics are parameters too. | §6 "documented detector types (peak/RMS), ballistics", "linked operation across multichannel signals". §6 names no de-esser ballistics; they need values, so they are parameters, not invented constants. |
| D30 | Limiter: required gain → sliding minimum over the look-ahead + 1 → average over the look-ahead (the attack) → exponential release. The output delayed by the look-ahead never exceeds the ceiling (sample peaks); `true_peak` detects on 4× oversampled peaks. Latency = look-ahead. | §6 "look-ahead, true-peak option"; a provable ceiling. |
| D31 | Filters are RBJ cookbook biquads and Butterworth cascades, in transposed direct form II in f64. Coefficients are recomputed every block while a parameter ramps. | §6 "biquads … with coefficient smoothing; designs documented with references". Recomputing per block keeps every intermediate filter a valid design, which interpolating coefficients would not. |
| D32 | Bypass crossfades the wet share over one block; the dry path is delayed by the node's latency. A bypassed node keeps running, so it can come back without a jump. | §6 "bypass" without clicks. |
| D33 | Taps pre/post-EQ and pre/post-dynamics: the input of the first and the output of the last node of that kind (EQ: `eq`, `filter`, `dynamic-eq`; dynamics: the compressors, limiter, expander, gate, de-esser, ducker, multiband). Without such a node both are the chain's input. `{"after": id}` taps after any node. A key reads another signal's tap, never its own. | §5 tap list. A key from the node's own signal could form a loop inside a block. |
| D34 | All signals are scheduled in one dependency order: sends and keys are edges, block-delay sends are not. A loop without a block-delay send is rejected and the message names the loop. | §4.4 feedback rule; keys need their source first. |
| D35 | Delay compensation per bus (`compensate`, default on): every send is delayed to the largest latency among the bus's sends. Keys and block-delay sends are left out. Outputs report `processing_latency_ms` and `latency_ms` = read offset + one block + processing (local inputs; mirrored ones add their extra delay). | §4.4, §12 "resulting latency per output". |
| D36 | Loudness per signal with `loudness: true`, post-fader. The DSP thread sends exact 10 ms segment energies (aligned to the sample index; 100 ms before the EBU file tests, see below); the control side gates and computes momentary, short-term, integrated and LRA. The meter lives in a DSP slot of its own, so the integration continues over plan swaps. A measurement runs from when it is switched on; stop, reset and start through the API. | §7 BS.1770 / R 128 / Tech 3342; "start/stop/reset … from UI, logic or API". Exact segments are what the gating needs. |
| D37 | True peak: a 4× polyphase interpolator of my own design (48-tap Kaiser-windowed sinc, β 6, every phase at unity DC gain), on every meter channel. BS.1770-4 allows other filters than its example. | §7 "true peak (ITU-R BS.1770 oversampling)". |
| D38 | A change of a chain's structure (a node added, removed, moved, or a type or structural parameter changed) fades the signal's fader, sends and output out and in around the swap; processors whose own structure did not change keep their state (filter memories, envelopes). | §2: no clicks, state kept for unchanged nodes. |
| D39 | After a missed deadline the DSP thread commits what it staged and writes silence over the output blocks it skips, and counts them in `dsp_deadline_misses_total`. | Found in testing (below): unwritten ranges kept the ring's samples of one buffer length earlier, which readers took for fresh audio. |
| D40 | Placed in later milestones: PPM ballistics and scales, the spectrum analyser, phase correlation and goniometer (§7) with the metering UI (M4); the oscillator (§6 utility) with the control room (M5); the loudness normaliser for outputs (§6) with auto-level (M7); ducking keyed by VAD (§8.4) with M7. | §19 M3 names EQ, filters, dynamics, de-esser, delay, delay compensation, true peak and loudness. These belong with their users. |

### Deviations

| # | Spec | Deviation | Reason |
| --- | --- | --- | --- |
| X2 | §4.4 / §6: a loop is accepted only with an explicit `block delay` node | The block delay is a property of a send (`block_delay: true`), not a chain node. | A loop always closes through a send, and the matrix is where an operator sees it. A node inside a chain would split a signal's processing into a part before and a part after the delay within one block. Same effect: an explicit delay of one block on the loop. |
| X3 | §6 multiband "linear-phase crossover option" | Not implemented yet; the multiband uses minimum-phase Linkwitz-Riley crossovers. | A linear-phase crossover needs long FIR filters (thousands of taps for low crossovers), so partitioned FFT convolution in the DSP thread, and adds tens of ms of latency. Proposed for the performance milestone (M10); question 3. |

### Results (dev VM)

- `cargo test`: 70 unit tests, including:
  - **filters:** every design matches its analog prototype on the warped axis
    (1e-6 dB); Butterworth −3.01 dB at the corner, 12–48 dB/oct;
  - **dynamics:** static curves (knee, ratio, expander, range); attack and release
    time constants (63 % within 0.3 dB);
  - **limiter:** never above its ceiling, latency = look-ahead;
  - **processors:** gate, expander and ducker levels; de-esser in both modes (only the
    sibilant band reduced); dynamic EQ; multiband flat within 0.01 dB at unity and
    compressing one band; delay; bypass and state handover;
  - **EBU Tech 3341** synthetic cases 1–5 (±0.1 LU) and the true-peak sines (fs/4 at
    0°, 45°, 60°, 67.5°: −6.0 dBTP within +0.2/−0.4);
  - **EBU Tech 3342** cases 1–4 (±1 LU);
  - **5.0 weighting** and LFE excluded;
  - **engine:** the delay-compensation null test (§17; uncompensated does not null);
    block-delay feedback (1, 0.5, 0.25, 0.125 per block).
- `tests/integration/processing.sh`, through MXL:
  - EQ +6.02 dB bell: L = 2 × tone, sample-exact;
  - EQ gain ramped to 0 dB without a plan swap;
  - bypass passes the input unchanged;
  - 48 dB/oct high-pass at 4 kHz: the 1 kHz tone at −117.8 dBFS;
  - compressor −30 dB 4:1: gain reduction 7.4 dB, peak −27.4 dBFS;
  - limiter at −26 dBFS: peak −26.000 dBFS; output latency 5.5 ms (2 + 2 + 1.5);
  - loudness: momentary, short-term and integrated −23.0 LUFS, max true peak
    −20.0 dBTP; stop, reset, start.
- `mix.sh`, `inputs.sh` and `lifecycle.sh` pass; AMWA in CI as in M2.
- **§16-shaped load** (release build, SCHED_OTHER on the shared 4-vCPU VM, block 96):
  - mix: 64 mono strips, each with HPF, 4-band EQ and compressor; de-esser on 16;
    32 stereo buses, 16 aux and 8 N-1 buses; 696 sends; 8 published outputs;
    true peak on all 120 meters;
  - over 60 s: mean block time 656 µs (`dsp_load_ratio` **0.33**, target < 0.5), all
    blocks ≤ 1.5 ms;
  - 0 deadline misses, 0 missing input samples, 0 output errors, commit lag 0.62 ms.

  VCAs (§16: 4) come with M6; the one-hour run on the reference CPU is for
  `tests/hw` with M10.

### Findings

- **Unwritten output ranges replay old audio.** After a missed deadline the DSP thread
  used to skip the late blocks without writing them. Readers of the output then
  found the ring's samples of one buffer length earlier (19 456 samples for a 200 ms
  history) and took them for fresh audio. Now the skipped blocks get silence and
  count as missed (D39). The same held for the test writer, which now catches up
  instead of skipping. The probe found it: 19 456 mod 48 = 16, so the stale tone was
  16 samples out of phase.
- Probes are retried once when the input was late or the DSP missed a deadline during
  them. On this shared VM that happens about once in ten runs; a gap without either
  still fails the test.

### Answers (maintainer, 2026-10-11)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Value ranges (EQ gain ±24 dB, Q 0.1–40, thresholds −100…0 dB, ratio ≤ 1000, knee 0–24 dB, makeup ±24 dB, look-ahead 0.1–20 ms, ceiling −40…0 dB, delay ≤ 2000 ms, ≤ 32 nodes) | Accepted. |
| 2 | Loudness tolerance ±1 LU around −23 LUFS; running when switched on | Accepted, both. |
| 3 | Linear-phase multiband crossover (X3) | In M10; ask again then. |
| 4 | EBU authentic test files | Download them in CI (below). |
| 5 | Merge PR 4 | Merged. |

### EBU test set (after the answers)

- `tests/ebu_loudness.rs` runs the EBU loudness test set v5 through the mixer's
  pipeline (segment meter in 96-sample blocks, gating, true-peak interpolator). CI
  downloads it (87 MB, cached), checks the official file's SHA-256 (`9cc500b4…b062`)
  and runs it with `cargo test`. Without `EBU_TEST_SET` the test is skipped.
- The download: tech.ebu.ch answers automated requests with a Cloudflare challenge
  (403). A GitHub release of the same zip, hosted with the EBU's permission
  (`maxwalley/EBULoudnessTestSetV5`), is used, verified by the hash.
- The expected values are those of Tech 3341 and Tech 3342. The `sdroege/ebur128`
  reference tests helped to read them per file.
- First run: everything passed except case 13-2 (maximum momentary −23.22 LUFS
  instead of −23.0 ± 0.1). Momentary loudness came every 100 ms, so a burst's
  maximum between two steps was missed. The DSP segments are now 10 ms (480
  samples, still aligned to the sample index): momentary and short-term every 10 ms,
  gating blocks and range values every 100 ms as before.
- Result: **Tech 3341 cases 1–23 and Tech 3342 cases 1–6 pass on the EBU files**:
  - integrated (1–8, incl. 5.0 and the 6-channel WAVE-EX file), ±0.1 LU;
  - momentary and short-term constant (1, 2, 9, 12) and maximum (10, 11, 13, 14),
    ±0.1 LU;
  - true peak (15–23), +0.2 / −0.4 dB;
  - loudness range (3342 1–6), ±1 LU.

## M4: console UI, processing chain editor, routing matrix; snapshots and show files

### What exists

- `web/`: the UI (Vue 3, Vite, one self-contained `index.html`), embedded by
  `build.rs` (a short note without a build). Tabs Console, Processing, Routing,
  Snapshots, Meters, NMOS, Status, Settings.
- `src/events.rs`: the WebSocket `/api/v1/events`.
- `src/mix/show.rs`, `src/show_api.rs`: layouts, snapshots (capture, filtered
  recall); `src/history.rs`: the show history.
- `src/dsp/ppm.rs` (PPM detectors), `src/dsp/fft.rs` (FFT, fractional-octave
  bands), `src/analysis.rs` (the analyser); correlation in the meter step.
- `scripts/demo.sh`, `tests/integration/show.sh`, `tests/integration/ui.sh` with
  `web/tests/ui.mjs`, `tests/perf/section16.sh`.

### Decisions

| # | Decision | Reason |
| --- | --- | --- |
| D41 | The UI is Vue 3 built by Vite into one `index.html` with everything inlined, embedded in the binary at build time (`build.rs`; without a build the binary serves a note, so Rust builds and tests need no Node). Base styles and components (`style.css`, pill, segmented control, pot) are copied from the sibling media functions. | §12.0: embedded, no CDN, the family look "copied, not redesigned". The siblings build the same way (vite-plugin-singlefile). |
| D42 | One WebSocket `/api/v1/events`: the status five times a second, the whole mix within 40 ms of a change of its revision, meters (and loudness) every 40 ms with a per-client subscription (signals, rate), the analyser on request. Every change goes through the REST API, rate-limited in the UI to 30 PATCHes a second per path with merged bodies. | §7 "decimated per client subscription"; §12.4 "the API covers every operator action": the UI uses nothing else. A whole mix per change is simpler than diffs and small (tens of kB for §16). |
| D43 | Multi-client sync: the mix comes back to every client after a change. The changing client applies its change to its copy at once (the next mix replaces it; a refused change reloads the mix). A control being dragged shows its own value; after a step it keeps the value it sent for 500 ms or until the server has it. | §12.1 multi-client sync without controls jumping under the operator's finger. Found in testing (below): quick steps and quick toggles were lost otherwise. |
| D44 | Snapshots hold parameters only: trim and polarity, fader, mute, pan and width, every chain node as JSON, sends' gain, mute and phase. They are part of the show. A recall matches by signal, node id and node type, skips what no longer exists, never touches recall-safe signals, and can be limited to signals, sections (strips, buses) and classes (trim, fader, mute, pan, chain, sends). `crossfade_ms` is the ramp of every value it changes. | §12.5. Structure (signals, sends, nodes) belongs to the show, not to a snapshot. A node's structural parameters (band types, slopes) are part of its JSON, so recalling them swaps the plan with the D38 fades. |
| D45 | Show history in `MIXER_STATE_DIR/history/`: an automatic version at most every 60 s after a change (written by the control tick when the change came earlier), the newest 100 automatic versions kept, named versions kept until deleted; restore and import keep the current show as a version first. | §12.5 "auto-save, versioned history". The interval and the count are mine (question 1). |
| D46 | Layouts: named pages of signal ids, part of the show; the console shows one page, or all strips in banks of 8–64 (a browser setting). | §12.1 "custom layouts (user-defined strip order and pages)", "layers/banks". |
| D47 | PPM: two IEC 60268-10 detectors (Type I: 5 ms integration, 20 dB in 1.7 s; Type II: 10 ms, 24 dB in 2.8 s) on every meter channel in the DSP thread, calibrated so that a steady 1 kHz tone reads its peak (the detector settles 0.2 dB below it). The UI draws the EBU, BBC, Nordic and DIN scales relative to an alignment level: EBU and Nordic TEST and BBC 4 at alignment, DIN 0 dB at alignment + 9 dB (the permitted maximum); alignment −18 dBFS (EBU R 68) by default, −20 dBFS (SMPTE RP 155) as option, a browser setting. | §7 "PPM ballistics (EBU, BBC, Nordic, DIN)". Ballistics are per type, so two detectors cover the four scales. Question 2 for the alignment. |
| D48 | Phase correlation of the first two channels of every signal with two or more, over the RMS window (300 ms). | §7 "phase correlation". |
| D49 | One analyser for the whole mixer: one tap point at a time, copied by the DSP thread into a ring (one second, both channels, no allocation); a 4096-point FFT (Hann) on the control side, 1/6-octave bands, correlation and 256 goniometer points every 40 ms, sent to the clients that ask. Changing the tap swaps the plan; no node changes, so nothing fades. | §7 "spectrum analyser on any tap point (computed on the control side from tapped blocks)". One shared tap keeps the DSP copy and the control CPU bounded (question 3). |
| D50 | Missed blocks in a loudness measurement count as silence up to 3 s (the DSP thread writes silence for them, D39); a longer gap restarts momentary and short-term. | Found in testing (below): a single missed deadline blanked momentary for 400 ms and short-term for 3 s. |
| D51 | Theme (system, light, dark), meter scale, alignment, bank size and recall options are kept in the browser (`localStorage`). | §12.0 "theme follows the system, switchable, remembered"; per operator, not per show. |
| D52 | The UI test drives the system Chrome through `playwright-core` (no browser download), with two clients, and checks every action on the API and on the other client. | CI runners have Chrome; §17 asks for UI checks. |

### Deviations

| # | Spec | Deviation | Reason |
| --- | --- | --- | --- |
| X4 | §7 "spectrum analyser on any tap point" | Any tap, but one at a time for all clients (D49). | Question 3. |

### Results (dev VM)

- `cargo test`: 81 unit tests (11 new: snapshots and layouts, history, PPM bursts
  and return times, the paired detectors, FFT against the direct DFT, band levels,
  the analyser, correlation, loudness gaps) and the 6 EBU groups.
- `tests/integration/show.sh`, through MXL:
  - the UI page at `/`;
  - a recall of strip faders only (bus mute kept); recall-safe kept at −10 dB; a
    full recall with a 400 ms crossfade, sample-exact afterwards;
  - snapshot rename and delete; layouts (a missing signal refused);
  - a named history version restoring a deleted send, sample-exact;
  - PPM Type I and II −20.0 dBFS on the −20 dBFS tone; correlation none with a
    silent channel, 1.0 with L = R;
  - analyser on the strip input: peak band 1000 Hz at −20.0 dBFS, correlation 1.0;
  - events: status 5/s, only the subscribed signal's meters at 25 Hz, the analyser,
    the mix again after another client's REST change.
- `tests/integration/ui.sh` (headless Chrome, two clients): mute; fader by keys
  (−2 dB) and by drag; the second client follows; EQ from the palette with its band
  handle dragged; compressor added, bypassed and moved; a send from the matrix; a
  snapshot; the analyser; the theme. Screenshots of every tab in the CI artifact.
- A manual walkthrough in the desktop browser and a recorded one (all tabs,
  crossfade recall) found no layout or update problems.
- `mix.sh`, `processing.sh`, `inputs.sh`, `lifecycle.sh` pass; AMWA in CI as before;
  the image serves the UI.
- **§16-shaped load** (`tests/perf/section16.sh`, release, SCHED_OTHER on the shared
  4-vCPU VM, back to back): M3 (`main`) mean block 694 µs; M4 790–874 µs
  (`dsp_load_ratio` 0.40–0.44, target < 0.5), all blocks ≤ 1.5–3 ms. The difference
  is PPM and correlation on all 120 meters; running both detectors in one pass
  halved their part (115 → 54 µs a block).

### Findings

- **A missed deadline blanked loudness.** A gap in the 10 ms segments restarted the
  windows. Now gaps count as silence (D50).
- **The history missed changes.** A change less than a minute after the last
  automatic version was never written unless another change came later; the control
  tick now writes it.
- **Quick steps were lost.** Four arrow-key steps before the server's answer landed
  as one, because each step started from the server's value (D43).
- **A quick second toggle was lost.** CI caught a mute pressed twice within the
  200 ms the mix took to come back: the second press sent "mute" again. The mix now
  goes out within one meter tick (40 ms) and the client applies its own changes at
  once (D43); the UI test presses mute twice quickly.
- **`inputs.sh` on a busy runner.** A writer 30 ms late gets an automatic delay of
  its p99.9 lag + 1 ms; one CI run lost a block when the writer was later than that
  once. The automatic delay then rises, as designed. Its missing-sample windows
  are now measured once more after a loss, like the probes (M3); a second loss fails.
- **PPM calibration.** With the return running between peaks, the charge time
  constants that make the integration-time bursts read −2 dB are 1.355 ms (Type I)
  and 2.763 ms (Type II), and a steady tone reads 0.2 dB low without the calibration
  gain.

### Answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Show history: an automatic version at most every 60 s, the newest 100 kept | Accepted. |
| 2 | PPM alignment −18 dBFS (EBU R 68), −20 dBFS as option; DIN 0 dB at alignment + 9 dB | Accepted. |
| 3 | One analyser tap shared by every client (X4), or one per client | One for now. |
| 4 | Snapshot crossfade ramps every changed value | Accepted. |
| 5 | Widgets (§12.6) and tally/AFV: next, or after M6 | Next, before M5. |
| 6 | Merge PR 5 and PR 6 | Merged. |

## Widgets and tally (1.0.0 set, before M5)

The maintainer chose to do the 1.0.0 widgets and tally/AFV next (M4 answer 5).
The widgets that need later milestones come with them: `monitor` and `talkback`
with the control room (M5), `vca` with M6, PFL in the strip widget with M5.

### What exists

- `src/widgets.rs`: `/widgets` and `/widget/<id>` (`strip`, `fader-bank`,
  `loudness`); `web/src/components/WidgetPage.vue`, compact strips.
- `src/tally.rs`: the TSL UMD 5.0 receiver; AFV in `src/control.rs` and the
  compiler; strip settings (tally index, AFV) and the tally panel in the UI.
- `tests/integration/tally.sh` (with `tsl_send.py`, an independent encoder),
  `tests/integration/widgets.sh` (with `web/tests/widgets.mjs`, `frame.html`).

### Decisions

| # | Decision | Reason |
| --- | --- | --- |
| D53 | The widget pages are the embedded UI page; it shows the widget its path names. `strip` takes a strip or bus id; `fader-bank` takes a layout page (`layout` by name, `page` by name or 1-based number) or `strips` (1–32 ids); `loudness` takes a signal id. Bad parameters (unknown ids, layouts or pages, a wrong theme) are 400. The params schema lists the current ids and layout names as enums. | Appendix B.2 and the siblings (`mxl-color-corrector`: one page, the widget from the URL). The designer can offer the ids as a choice. A bank is a layout page (maintainer, question 4). |
| D54 | `/widgets` CORS and the page CSP as in the siblings: an origin is listed when a source of `WIDGET_FRAME_ANCESTORS` equals it (case and a trailing slash ignored) or is `*`; no API CORS on these routes; `;`, `,` and control characters in the setting exit with 78. | Appendix B.2 ("two Access-Control-Allow-Origin values make browsers refuse it"). |
| D55 | The TSL receiver is the one of mxl-webrtc-monitor ported: DLE/STX framing (DLE DLE, DLE/ETX accepted, a packet complete at PBC + 2), bare or wrapped UDP datagrams, UTF-16 labels, at most 8 TCP senders, `TSL_SCREEN`. Default ports 8910/8911 as §10.3 says. | §10.3 "as in mxl-multiviewer and mxl-webrtc-monitor". The monitor moved its defaults to 8912/8913 because it runs with host networking next to a multiviewer; the mixer runs on the pod network (question 1). |
| D56 | `TSL_MAP` maps a TSL display index to a tally index (`display:index`; empty: the same number; with a map, unmapped displays are ignored). Strips carry their `tally_index` in the show. | The siblings map displays to their own channels; the mixer's strips are show data, so the deployment maps displays and the show maps strips. |
| D57 | AFV is a gain after the strip's fader, present only while AFV is enabled: open (1) while the index is on air (LH lit), at `depth_db` otherwise, also before any tally arrived; the gain ramps linearly over `fade_in_ms` or `fade_out_ms` (0–10 000) without a plan swap. Post-fader sends, outputs and meters follow it; pre-fader sends do not. | §10.3: "opens when its mapped tally index is on air (LH), and closes otherwise". The engine has no defaults (D28); the UI proposes −∞, 100 ms, 500 ms (question 2). |
| D58 | The last tally state stays when packets stop (no timeout); the NMOS tab shows each index's age and the time since the last packet. | Closing every AFV strip because the tally calculator restarted would cut the programme; question 3. |

### Results (dev VM)

- `cargo test`: 88 unit tests (7 new: TSL packets with UTF-16 labels, framing
  with escapes across reads, screen and map, the AFV node in the compiler, widget
  origins and parameters, the settings).
- `tests/integration/tally.sh`, through MXL:
  - an AFV strip without tally is closed (silent output);
  - index 3 on air over UDP with a UTF-16 label ("Kamera 3 Zürich"): open,
    sample-exact; a message for another screen is ignored;
  - on preview only over TCP (DLE/STX): closed;
  - a 1 s fade in: −28.6 dBFS after 0.4 s (gain ≈ 0.37), −20.0 dBFS after 1.2 s;
  - AFV removed: the strip follows its fader; `tally_packets_total` counts.
- `tests/integration/widgets.sh`: the list (CORS for the listed origin only,
  `Vary: Origin`, 405 with `Allow: GET`), the pages (CSP, no `X-Frame-Options`,
  400/404), and in headless Chrome: each widget framed from the allowed origin
  posts `widget-ready` and `widget-size`; from a refused origin the frame is
  blocked and nothing arrives.
- `tests/integration/ui.sh`: tally index and AFV set in the strip settings, index
  on air over UDP: AFV opens, the strip is lit red on both clients, the NMOS tab
  lists the lamp.
- All other integration tests pass.

### Answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | TSL default ports 8910/8911 (§10.3) | Accepted. |
| 2 | AFV defaults in the UI: depth −∞, fade in 100 ms, fade out 500 ms | Accepted. |
| 3 | AFV closed before any tally; the last state stays when packets stop | Accepted, both. |
| 4 | `fader-bank`: banks in mix order, or a layout page | A layout page: `layout` and `page` (name or 1-based number) replace `bank` and `size` (D53). |
| 5 | Merge PR 7 | Merged. |

After the merge, `main` CI failed once in `show.sh`: the correlation read 0.9969
0.5 s after the pan change, because a late meter tick kept the pan ramp in the
300 ms window. The test now waits up to 3 s for the steady value.

## M5: control room

In three parts, each a PR:
- **M5a** (merged, PR 9): the monitoring engine and UI: monitors, solo, checks, MXL
  monitor outputs, the oscillator, the `monitor` widget, PFL in the strip widget.
- **M5b** (PR 10): the preview contract (Appendix B.1): MediaMTX (own child or shared),
  monitors as Opus over RTSP (an RTSP publisher of my own, `libopus`; no GStreamer),
  WHEP listening in the UI, personal monitors (`MIXER_MAX_WEB_MONITORS`), encoding
  only what someone listens to, and the latency measurement (§3: ≤ 150 ms
  glass-to-ear).
- **M5c**: talkback (an MXL input or the browser microphone over WHIP), its
  destinations with control-room dim, IFB/N-1 interrupt with priority, listen-back,
  the `talkback` widget.

### M5a: what exists

- `src/mix/room.rs`: the control room in the show; `src/room_api.rs`; the solo
  state in `Control` (runtime).
- `src/mix/compile.rs`: the solo bus, every monitor path, solo-in-place;
  `oscillator` in `src/mix/chain.rs` and `src/engine/proc.rs`.
- UI: the Control room tab, PFL/AFL on strips, `MonitorCard.vue`, the `monitor`
  widget.
- `tests/integration/room.sh`.

### M5a: decisions

| # | Decision | Reason |
| --- | --- | --- |
| D59 | A monitor path: sum of its source (strip post-pan, bus post-fader, or an input's channels 1–2, width-mapped to the monitor's format) and, while anything is soloed and it follows solo, the solo bus; then the L/R check (a 2×2 matrix, other channels unchanged), the level (level, dim, cut, trim), its alignment chain, its delay, its meter, its output. Every control is a smoothed value: no plan swap. | §10.1 monitor controls and speaker sets with trim, delay, EQ. |
| D60 | The checks are one choice (`stereo`, `mono`, `difference`, `swap`, `left`, `right`): together they would contradict each other. Mono and difference use the mix's stereo-to-mono sum. | §10.1 "mono check, phase check (L−R), L/R swap, solo L / solo R". |
| D61 | Solo: the solo bus is stereo; soloed signals send into it (PFL from the pre-fader tap with `pfl_trim_db`, AFL from the output tap), width-mapped (mono centred with the pan law). Who is soloed is runtime state, not part of the show; a change swaps the plan, the new sends fading in, nothing else touched. | §10.1 solo system. Solo is operating state; after a restart nothing is soloed. |
| D62 | The automatic PFL dim: a monitor that follows solo plays its programme at `programme_db` while anything is soloed (default −21 dB; `-inf` replaces it). Dim depth default −21 dB. | §10.1 "automatic PFL dim of the main monitor". The defaults are the maintainer's (answer 1). |
| D63 | Solo-in-place (`sip`, off by default): while anything is soloed, every strip that is neither soloed nor `solo_safe` gets fader 0 in the mix. | §10.1 "SIP … destructive; requires explicit enable and a solo-safe list". |
| D64 | The oscillator is a node type (`oscillator`: sine or the EBU R 49 stereo ident, coherent on every channel, the left channel off for 0.25 s every 3 s with 2 ms ramps), usually alone on a bus. A bus goes anywhere: sends, monitor sources, published outputs. | §10.2 "oscillator to any destination … tone presets, EBU ident"; no new kind of signal. Presets in the UI: 1 kHz −18 dBFS (R 68), 1 kHz −20 dBFS, the ident. |
| D65 | A published monitor is an MXL output with an NMOS sender "Monitor <name>" and flow id UUIDv5 of `monitor:<id>` (a colon never appears in a signal id). | §10.1 "each speaker set/headphone path is published as an MXL audio flow". |
| D67 | Bass management per monitor, switched on and off in the control room: for formats with an LFE channel, the other channels high-passed and their sum low-passed into the LFE (Linkwitz-Riley 24 dB/oct: two Butterworth sections each way, so the halves sum flat), the LFE input at `lfe_gain_db`; after the alignment chain, before the delay. The UI proposes 80 Hz and +10 dB. Stereo monitors have no sub channel. | Maintainer, answer 2. 80 Hz and +10 dB (the LFE's in-band gain) are common practice, not from the spec (question below). |
| D66 | CI runs the integration tests on release binaries. | A debug build's DSP thread is about 15 times slower (load 0.2 against 0.013 for the test mix); on shared runners that missed deadlines. The image ships release binaries. |

### M5a: results (dev VM)

- `cargo test`: 92 unit tests (4 new: the oscillator and the ident, the room
  model and its validation, checks and monitor gain, bass management).
- `tests/integration/room.sh`, through MXL:
  - a published monitor on the programme bus, sample-exact; its NMOS sender
    "Monitor Main";
  - level −20 dB, dim, cut, swap, left only: sample-exact;
  - PFL of a strip with its fader at −∞, centred at −3 dB, programme replaced; AFL
    post-pan; interlock;
  - solo-in-place: a non-soloed strip muted in the programme, kept when
    solo-safe, everything back afterwards;
  - an input as the source; an oscillator bus at 1 kHz −18 dBFS on the monitor.
- `ui.sh`: PFL from a strip shown in the Control room tab, the monitor's dim;
  `widgets.sh`: the `monitor` widget framed.
- **Finding:** probes failed now and then when the input's automatic delay
  changed during them (the change fades the input). The probe retry now also
  watches the input's effective delay.

### M5a: answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Defaults (dim, programme under solo, presets, level range, 16 monitors) | Dim −21 dB and the programme under solo −21 dB; the others accepted. |
| 2 | Bass management | A bass management that is switched on and off in the control room (D67). |
| 3 | Solo state not saved | Correct. |
| 4 | Merge M5a before M5b | Merged. |

After the answers: bass management (D67), unit-tested (1 kHz stays on L, 30 Hz on
the LFE, the crossover sums flat within 0.05 dB at 80 Hz, the LFE input at +10 dB)
and in `room.sh` (a 5.1 monitor on the oscillator: at 30 Hz the LFE carries L + R
at −12.1 dBFS, L below −45 dBFS; off again: 30 Hz on L).

### M5b: what exists

- `src/preview/`: the streams (`mod.rs`: on demand, an Opus encoder thread per
  stream), the RTSP publisher (`rtsp.rs`), the built-in MediaMTX (`mediamtx.rs`:
  configuration, supervisor, port check, reader counts).
- `src/preview_api.rs`: `/api/v1/preview`, `/api/v1/preview/<id>/listen`,
  `/api/v1/personal…`.
- Engine: `Step::Stream` copies a monitor's stereo block into the ring of a stream
  slot (`Command::SetStream`, `ClearStream`); the compiler adds it for every
  monitor with an active stream and for personal monitors.
- The `second-pulse` oscillator waveform.
- UI: `web/src/whep.js` (the WHEP player), Listen on the monitor cards, "My
  headphones" (`PersonalCard.vue`), the preview in the Status tab.
- `tests/integration/preview.sh` with `web/tests/preview.mjs`; Listen and the
  status in `ui.mjs`. CI downloads MediaMTX; the image has it.

### M5b: decisions

| # | Decision | Reason |
| --- | --- | --- |
| D68 | An RTSP publisher of the mixer's own: OPTIONS, ANNOUNCE (SDP: Opus/48000/2, PT 96), SETUP with TCP interleaving, RECORD; RTP per RFC 7587. A lost connection is reopened every 2 s. | §3 "RTSP with a small Rust client"; no GStreamer. TCP: no UDP ports to allocate, and RTSP runs on 127.0.0.1 or inside the cluster. B.1 "reopening by itself … every ~2 s". |
| D69 | Opus through the `opus` crate (libopus 1.5, built from source by `opusic-sys` and linked statically, BSD-3-Clause): stereo, application LowDelay, 10 ms frames, 128 kbit/s. Other monitor formats are downmixed to stereo with the width matrix. | §10.1 "WHEP, Opus, stereo, low-latency settings". 128 kbit/s and 10 ms are my choice (question 1). |
| D70 | The DSP thread copies a stream's block into an SPSC ring (2 s; dropped when full, never waits); an encoder thread per stream does the rest. At most 96 streams. A stream that starts or stops changes the plan structure, like a send. | Real-time rules: no I/O or allocation in the DSP thread. |
| D71 | On demand: a stream runs while it is listened to (the UI repeats `POST …/listen` every 5 s) or, with `MEDIAMTX_API_URL`, has readers; 10 s after the last, it stops. | §10.1 "only paths someone listens to are encoded … stops encoding after a grace time". The built-in MediaMTX has no API (G6), so the listen requests are needed. 10 s: question 1. |
| D72 | The built-in MediaMTX: a child process with a rendered configuration (`MIXER_STATE_DIR/mediamtx.yml`, working directory the state dir), restarted after 1 s, doubling to 30 s (back to 1 s after a minute up); its ports checked before the start (exit 75); killed and reaped at shutdown. Only what the mixer needs: RTSP on 127.0.0.1 (TCP), WebRTC with `NMOS_HOST_ADDRESS` as an ICE host; no API, metrics, HLS, RTMP, SRT or MoQ; only `<prefix>/…` paths; publishing only from 127.0.0.1/::1, reading from anywhere. | B.1, G6, §18. MediaMTX's defaults let anyone publish to any path (WHIP from the network), and v1.20 starts a MoQ server that writes certificates into the working directory. |
| D73 | MediaMTX v1.20.1, the release tarball checked by SHA-256 (amd64, arm64), in CI and the image (`/usr/local/bin/mediamtx`, its licence under `/usr/share/doc/mediamtx`). | CI avoids Docker Hub (the AMWA job already does); the tarball is the upstream release. MIT. |
| D74 | Personal monitors: runtime state, not in the show; ids `personal-<n>`; their own source, level, dim, cut, check and solo (their own solo bus); at most `MIXER_MAX_WEB_MONITORS` (default 8, 0–64); removed after 10 min unheard. The browser keeps its id for the tab (`sessionStorage`). | §10.1 "each logged-in UI client can have its own PFL and headphone selection without affecting the control room speakers". The mixer has no login: one per browser tab. Defaults: question 1. |
| D75 | The latency is measured end to end: the `second-pulse` oscillator (the tone in the first 20 ms of every TAI second, from the block's sample index) on a monitor; headless Chrome plays it by WHEP, an AudioWorklet finds each onset, `AudioContext.getOutputTimestamp` maps it to the wall clock; the latency is that time modulo 1 s. | §3: "target ≤ 150 ms glass-to-ear … record the measurement". TAI and UTC seconds differ by whole seconds; the mixer and the browser share the clock. |
| D76 | Every change answers with `x-mix-revision`; the UI keeps its own unacknowledged changes over pushed mixes older than that, and sends at most one change per path at a time. | A finding (below): a toggle pressed twice quickly could stay on. |

### M5b: results (dev VM)

- `cargo test`: 96 unit tests (new: the settings, the MediaMTX configuration and
  reader counts, the RTSP SDP and RTP, the second pulse).
- `tests/integration/preview.sh`:
  - own mode: the child started; WHIP from the host's network address refused
    (401), a path outside the prefix not served (400);
  - monitor `main` in headless Chrome over WHEP: 12 s of audio, a pulse every
    second; the stream stopped 10 s after the last listen request;
  - the child killed (SIGKILL): restarted with a new pid, the stream published
    again and playing;
  - a personal monitor played, the page and WHEP on the host's address;
  - the child ends with the mixer (exit 143);
  - shared mode (a separate MediaMTX, `PREVIEW_PUBLISH_URL`, `PREVIEW_WHEP_URL`,
    `MEDIAMTX_API_URL`): no child, the stream on the shared MediaMTX and playing; an
    RTSP reader on it (ffmpeg) kept the stream for 14 s without listen requests;
    without it the stream stopped.
- **Latency** (media time to the browser's audio output, Chrome's output latency 32
  ms included), median per run over 6–12 pulses, in seven runs: 82.4–98.5 ms for
  `main` (own mode, also after a child restart), 78.8–93.8 ms for a personal
  monitor, 95.1 and 99.1 ms in shared mode; single pulses 75.2–104 ms. Below 150
  ms: no in-process WebRTC stack (§3).
- `ui.sh`: Listen on a monitor card until "Listening", the stream publishing in
  the Status tab, stopped; two mute presses around a mix pushed 1 s late (below).
- All integration tests pass on release binaries; the image starts the child (uid
  1000, `/config/mediamtx.yml`) and exits 143.

### M5b: findings

- **A toggle pressed twice quickly could stay on.** The UI changed its copy at
  once and the next pushed mix replaced it. A mix pushed before the server had the
  first press (after another change) arrived between the presses, so the second
  press toggled from the old value. Seen in `ui.sh` under load; `ui.mjs` now holds
  the pushed mixes back by 1 s and fails without the fix (D76) every time.
- **The next start found the child's ports taken:** the mixer exited before the
  supervisor had killed and reaped MediaMTX; shutdown now waits for it.
- **A demo mixer on the same machine fails the probes:** every mixer pins its DSP
  thread to the last CPU it may use, so two share a core and the inputs come late
  (`main` fails the same way). Documented in `docs/development.md`; CI runs one
  mixer at a time.
- The measuring AudioWorklet needs a secure context, which the host's address over
  http is not; the test lets Chrome treat that origin as secure. The UI's player
  (RTCPeerConnection, `<audio>`) does not need it.

### M5b: questions

| # | Question | Proposal |
| --- | --- | --- |
| 1 | Defaults: Opus 128 kbit/s and 10 ms frames; 10 s grace; listen repeated every 5 s; `MIXER_MAX_WEB_MONITORS` 8; a personal monitor removed after 10 min unheard. | As implemented; none of them is in the spec. |
| 2 | Browser talkback (M5c) is published by WHIP; the built-in MediaMTX takes publishing only from this host. | Allow publishing from anywhere only on `<prefix>/talkback/…` paths (the mixer only reads them), and only in own mode; the shared MediaMTX is the platform's. |
| 3 | The deployment example's `PREVIEW_*` values (`rtsp://mediamtx:8554`, `production/mix1`, `https://preview.example`, `http://mediamtx:9997`) are placeholders. | Tell me the platform's names, or leave them as examples. |
| 4 | Merge M5b (PR 10) before M5c? | Yes, as with M5a. |

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
