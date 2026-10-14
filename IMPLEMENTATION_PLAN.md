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
| 8 | Com-IS-Assist (GPLv3) in this MIT repository | Copy it. The maintainer knows its developer: either that project changes its licence or this one does. The copy comes with milestone 7 (§8.2), with attribution and its licence noted; the licence question is a release blocker for the release that contains it until it is settled. (2026-10-12: settled: the mixer stays MIT and Com-IS-Assist's core is MIT now; see “Licence” at the end.) |

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
- **M5c** (PR 11): talkback (an MXL input or the browser microphone over WHIP), its
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
    `MEDIAMTX_API_URL`): no child, the stream on the shared MediaMTX and playing; a
    WHEP reader kept it for 20 s after a single listen request (its reader count
    from the API); without it the stream stopped.
- **Latency** (media time to the browser's audio output, Chrome's output latency 32
  ms included), median per run over 6–12 pulses, in seven runs: 82.4–98.5 ms for
  `main` (own mode, also after a child restart), 78.8–93.8 ms for a personal
  monitor, 95.1 and 99.1 ms in shared mode; single pulses 75.2–104 ms. On a
  GitHub runner: 78.6, 96.3, 96.6 (personal) and 94.7 ms (shared). Below 150 ms:
  no in-process WebRTC stack (§3).
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

### M5b: answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Defaults (Opus 128 kbit/s, 10 ms; 10 s grace; listen every 5 s; 8 web monitors; personal monitors removed after 10 min) | Accepted. |
| 2 | Browser talkback with the built-in MediaMTX | Accepted: publishing from anywhere only on the talkback paths, own mode only (M5c). The paths are §10.2's `<prefix>/talkback-<client>`. |
| 3 | `PREVIEW_*` values in the deployment example | Kept as examples. |
| 4 | Merge M5b before M5c | Yes. |

**Licence (Com-IS-Assist, §8.2):** the mixer stays MIT. Whether Com-IS-Assist's
core can be used under a permissive licence is an open discussion between the
maintainer and its author. Its own licensing notes say GPLv3 comes only from the
VST3 target (`vst3-sys`) and that the core crate could be licensed separately.
Until it is settled, no Com-IS-Assist code is copied; answer 8 of M2 (a release
blocker for the release that contains it) still holds.

### M5c: what exists

- `src/mix/talkback.rs`: mics, destinations, the gains of a destination (mix,
  interrupt, priority), validation; `control_room.talkback` in the show, the
  monitors' `talkback_dim_db`; a strip channel's patch can be `{"talkback"}`.
- `src/mix/compile.rs`: a mono buffer and a meter (`talkback:<id>`) per mic;
  after a destination bus's fader, its programme gain and a send from every mic;
  the talkback dim in the monitor level; listen-back as the source of the
  monitors that follow solo.
- `src/engine/talk.rs`: 16 talkback slots (engine inputs after the MXL inputs)
  with a jitter buffer.
- `src/preview/rtsp.rs`: an RTSP reader; `src/preview/talkback.rs`: browser mics
  (registration, an RTSP reader and Opus decoder per mic).
- `src/talkback_api.rs`; the talk state in the status.
- UI: `web/src/talkback.js`, `TalkbackPanel.vue` in the Control room tab, the
  talkback dim and lamps on the monitor cards, talkback mics in the strip patch,
  the `talkback` widget.
- `tests/integration/talkback.sh` with `web/tests/talkback.mjs`;
  `mxl-mixer-probe --onsets`; talkback in `ui.mjs`, the widget in `widgets.sh`.

### M5c: decisions

| # | Decision | Reason |
| --- | --- | --- |
| D77 | Talkback mics: an MXL input channel each (level, priority 0–9, `dims_control_room`), and browser microphones as runtime mics `browser-<n>` (priority `browser_priority`, never dim). Destinations: one per bus, talk mixed in after the bus fader at the destination's level; `mix` adds it to the programme, `interrupt` takes the programme to `interrupt_db` (default `-inf`) and lets only the highest-priority talking mics through. While a mic that dims the control room talks, every control-room monitor is at its `talkback_dim_db` (default −21 dB, like the dim). | §10.2: destinations "studio, IFB/N-1 feeds, remote, auxes" are buses here (N-1s will be too, M6); "interrupt or mix into N-1 feeds, with per-feed priority" (question 1); §10.1 "talkback dim". |
| D78 | Every mic-to-destination path is in the plan while both exist; talk only changes their gains (10 ms ramp). A new mic or destination changes the plan; pressing talk never does. | Talk must be immediate and click-free. |
| D79 | Latching or momentary is the client's: a momentary talk is repeated every second while held; the mixer ends one not repeated within 3 s, so a client that goes away never leaves talk on. The UI: a press shorter than 400 ms latches (or releases a latch), a longer one talks while held. | §10.2 "latching or momentary". |
| D80 | Browser talkback: a tab registers its microphone (`POST /api/v1/talkback/browser`, repeated every 5 s, removed 30 s after the last) and publishes it by WHIP as `<prefix>/talkback-<client>`. The mixer reads it over RTSP (interleaved TCP, an OPTIONS keepalive every 20 s), decodes the Opus (concealing up to 5 lost frames) to mono into an engine slot with a jitter buffer (20 ms, +10 ms per underrun, at most 100 ms; more than 50 ms over the target is dropped). In own mode the built-in MediaMTX takes publishing from the network on these paths only. The browser asks for the microphone with echo cancellation and noise suppression, without automatic gain. | §10.2, B.1; maintainer's answer 2 of M5b. The jitter buffer is the talkback signal's "own measured delay". |
| D81 | Listen-back is runtime state: the monitors that follow solo play a destination's return (or any source) instead of their own source. | §10.1, §10.2 "listen-back of any returning feed on the monitor". |
| D82 | A strip channel can be patched to a talkback mic, a browser's too. | §5 "talkback … every signal can feed the matrix"; §17 "WHIP talkback reaches a strip". |
| D83 | The browser talkback's latency: Web Audio plays a burst at every wall-clock second into the published stream; `mxl-mixer-probe --onsets` finds the bursts in the mixer's MXL output; the latency is their position in the TAI second. | The mixer and the browser share the clock (as in D75). |

### M5c: results (dev VM)

- `cargo test`: 101 unit tests (new: talkback gains with interrupt and priority,
  their validation, the compiled talkback paths with a browser mic, the jitter
  buffer, the RTSP reader's SDP and RTP parsing).
- `tests/integration/talkback.sh`, sample-exact through MXL:
  - the IFB bus without talk: the programme; the director talking: the
    programme cut, the director heard, the monitor at −21 dB;
  - the director outranks the producer; the producer alone is heard and does not
    dim the monitor;
  - mix mode: programme and producer together (−20 dBFS RMS of two −23 dBFS
    tones), the producer alone with the programme muted;
  - a momentary talk not repeated ended after 3 s;
  - listen-back of the IFB's return (input 1) on the monitor, and back;
  - a strip on the director's mic;
  - a browser microphone from headless Chrome by WHIP, a strip on it into an MXL
    output: a burst every second, latency (from Chrome's rendering of the burst to
    the MXL output) median 50.5 and 44.2 ms in two runs, 40.5–55.9 ms over 16
    bursts (on a GitHub runner: median 46.2 ms); the mic removed 30 s after its
    last registration.
- `ui.sh`: a talk button latched by a short press (the monitor's talkback lamp),
  released; held, it talks only while held; listen-back from the panel.
  `widgets.sh`: the `talkback` widget framed.
- **Finding:** a centred mono strip at its post-pan tap into a mono bus has a gain
  of 1.0012 (+0.01 dB): the pan law's −3 dB is 1/√2, the stereo-to-mono sum's
  −3 dB is 0.7079. The tests send post-fader where they compare sample-exactly.

### M5c: questions

| # | Question | Proposal |
| --- | --- | --- |
| 1 | "Per-feed priority" (§10.2): a priority per mic (on an interrupting feed only the highest talking priority is heard), or a priority per mic and feed? | Per mic, as implemented; per feed later if needed. |
| 2 | Defaults and ranges: talkback dim −21 dB, interrupt cuts (`-inf`), mode `mix`, mic and destination level −40 … +20 dB, priority 0–9, momentary hold 3 s, long press 400 ms, browser mic removed after 30 s, jitter buffer 20–100 ms, echo cancellation and noise suppression on, AGC off; at most 16 mics, 32 destinations, 16 browser mics. | As implemented; none is in the spec. |
| 3 | One destination per bus. | Yes; N-1 feeds (M6) are buses, so they become destinations then. |
| 4 | Talkback from browsers without authentication or roles (§20 open point): anyone who reaches the UI can talk. | Leave it for the platform's access control now. |
| 5 | Merge M5c (PR 11)? M5 is then complete; 1.0.0 still needs M6. | Yes. |

Maintainer, 2026-10-12: merge approved (question 5). Questions 1–4 are still open.

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

## Licence: MIT (maintainer, 2026-10-12)

The same day, after Andy Weiss relicensed Com-IS-Assist (MIT for every crate but
the VST3 wrapper, which stays GPL-3.0-or-later because of `vst3-sys`; commit
`f8f6aae`), the maintainer decided to go back to MIT: audio companies shall be
free to take from the mixer. master_me and PodcastPlugins are not copied, only
taken as inspiration.

| # | Decision | Reason |
| --- | --- | --- |
| D85 | MIT again: the MIT `LICENSE` (as before), `license = "MIT"` in the crates, the image label, README, §3 of the spec. D84 is reverted. `main` was GPL-3.0-or-later from PR 12 to this change, with no release and no code from others in between: the author's own code, which the author licenses as MIT. | The maintainer's decision, with Com-IS-Assist's author. |

- **Com-IS-Assist** (`com-is-assist-core`, MIT, Andreas Weiss): milestone 7 can
  take its code. Copied files keep its copyright and permission notice (MIT
  requires them in copies) and say where they come from. Its VST3 crate (GPL) and
  the LGPL Reaper JSFX reference stay out.
- **master_me, PodcastPlugins** (GPLv3): inspiration only. No code, and no
  line-by-line port of their Faust DSP (that would be a derived work); the mixer
  builds the experience from its own nodes, from the ideas and the published
  literature. Comparing behaviour (measuring their output against ours) is fine.

The GPL period, as it was decided and then taken back:

### GPL-3.0-or-later (maintainer, 2026-10-12, reverted by D85)

The maintainer decided with others: the mixer is open source and every fork
shall stay so. It is licensed under the GNU General Public License, version 3 or
any later version, instead of MIT. Code can no longer go from this repository into
the MIT sibling repositories (only the author's own code, by the author); MIT code
from them can still come in. No release was made under MIT.

| # | Decision | Reason |
| --- | --- | --- |
| D84 | `GPL-3.0-or-later`: the GPLv3 text as `LICENSE`, the `license` field of the crates (`Cargo.toml`, `spikes/m0`), the image's `org.opencontainers.image.licenses`, README, CHANGELOG, §3 of the spec. No per-file headers: the licence is the repository's (question 2). | The maintainer's decision. "Or later", as the FSF recommends and as PodcastPlugins is licensed, keeps later GPL versions open; GPLv3-only code (master_me, Com-IS-Assist say "GPLv3") can still be combined, the combination is then distributed under v3 (question 1). |

**Dependencies checked** (`cargo metadata`, all crates in the build): MIT,
Apache-2.0, BSD-2/3-Clause, ISC, Zlib, BSL-1.0, Unicode-3.0, CC0/MIT-0,
CDLA-Permissive-2.0 (not in the binary): all compatible with GPLv3. aws-lc (crypto
of `mxl-nmos`'s HTTP client, in the binary) has OpenSSL- and SSLeay-derived code;
its licence file puts it under Apache-2.0, which GPLv3 accepts. libopus is
BSD-3-Clause; MediaMTX (MIT) is a separate executable in the image; the UI's
packages are MIT.

**What it opens up:**
- **Com-IS-Assist** (Andy Weiss, GPLv3, Rust): §8.2 can take its code (the core
  crate: automix, loudness, voice activity) in milestone 7. Copied files keep
  its copyright notices, say where they come from and that they were changed
  (GPLv3 §5a); its LGPL Reaper JSFX reference stays out. Answer 8 of M2 is
  settled.
- **master_me** (Klaus Scheuermann, GPLv3, Faust): automatic mastering for live
  streams: gate, EQ, a leveler (gain to a loudness target with a brake), knee
  compressor, an 8-band multiband compressor, limiter, LUFS metering, no latency.
- **PodcastPlugins** (the same author, GPLv3+, Faust): speech enhancement, a
  `Track` chain (level, EQ, dynamics per speaker) and a `Master` chain.

  Both compare to §6 (processing nodes: EQ, compressor, multiband, limiter
  exist since M3) and §8.2 (speech-aware auto-level, M7). Proposal: compare them
  in M7 against the existing nodes and the auto-level plan; take over what the
  mixer lacks (the leveler with its brake as the auto-level controller, the
  track/master chains as presets of existing nodes). The DSP is Faust: either port
  it by hand to Rust nodes, or generate Rust with the Faust compiler
  (`faust -lang rust`) and wrap it as a node (a build-time tool; question 3).
  Their results can be reference tests for ours.

### Licence: answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | GPL-3.0-or-later or GPL-3.0-only? | Or-later. |
| 2 | SPDX lines in every source file? | Not now; when code from elsewhere comes in. |
| 3 | master_me and PodcastPlugins: port by hand or generate with Faust? | Plan the comparison. The mixer has most of the tools already; let them inspire a similar experience (below). |
| 4 | A NOTICE file for the sibling repositories' MIT parts? | No. |
| 5 | Merge this and M5c. | Yes. |

### Planned for milestone 7: the master_me / PodcastPlugins experience

Most of their tools exist (EQ, compressor, multiband, limiter, de-esser, gate,
loudness since M3); what they offer beyond them is the experience: a chain that
works out of the box, a few controls that matter, and automatic level. M7 adds,
inspired by them and built from the mixer's own nodes:
- **The comparison first:** for each of their modules (master_me: gate, EQ,
  leveler, knee compressor, multiband, limiter, LUFS; PodcastPlugins `Track` and
  `Master`) what the mixer has, what differs (parameters, ballistics, defaults),
  what is missing. Recorded here, with a measurement against their output where
  it helps.
- **Chain presets:** a speech track chain and a master chain like theirs, made of
  existing nodes with their kind of defaults, one click on a strip or a bus.
- **A leveler:** gain riding towards a loudness target with a brake (it is the
  §8.2 auto-level controller), with Com-IS-Assist's speech/bed logic beside it.
- **A simple view:** for such a chain, the few controls (target, amount,
  character) instead of every node's parameters; the full chain stays editable.
- No code from master_me or PodcastPlugins (D85): the experience is built from
  the mixer's own nodes; their behaviour is compared by measurement. Com-IS-Assist
  (MIT) code may be taken, with its notice.


## M6: VCA/DCA, mute groups, N-1 and group operations

In two parts, each a PR:
- **M6a** (PR 14): VCAs (nested, mute, PFL/AFL, spill, snapshots), mute groups
  with mute-safe signals, the `vca` widget.
- **M6b** (PR 15): the high-level operations of §5: group (strips → a subgroup bus with
  routing), N-1 (per participant or one of a selection; a first-class signal),
  stereo pair / surround link (shared processing and fader), mix-minus from a bus
  ("bus minus X"); N-1s as talkback destinations; the §17 N-1 correlation test.

### M6a: what exists

- `src/mix/vca.rs`: VCAs and mute groups in the show (`vcas`, `mute_groups`), the
  VCAs above a signal and the signals under a VCA, the effect on each fader,
  validation; `mute_safe` on strips and buses.
- `src/mix/compile.rs`: every strip's and bus's fader node takes its VCAs and mute
  groups; a VCA's solo is its members'.
- `src/mix/show.rs`: VCAs in snapshots (section `vcas`).
- `src/group_api.rs`: `/api/v1/vcas`, `/api/v1/mute-groups`.
- UI: `VcaStrip.vue` (beside the buses), `GroupsPanel.vue`, mute group buttons in
  the console toolbar, spill, VCA chips and group mutes on the strips, mute-safe
  in the signal settings; the `vca` widget.
- `tests/integration/vca.sh`; VCAs and mute groups in `ui.mjs`, the widget in
  `widgets.sh`.

### M6a: decisions

| # | Decision | Reason |
| --- | --- | --- |
| D86 | A VCA lists its members (strips, buses, VCAs). A member's fader node gets its fader plus the `fader_db` of every VCA above it, each counted once even when reached twice; one at `-inf` silences it. The sum is capped at +15 dB (answer 1). VCA ids share the signals' namespace (a solo or a layout entry names either). | §9 "effective member gain = member fader + sum of all assigned VCA offsets (dB), nested VCAs allowed (cycles rejected)". |
| D87 | A muted VCA, and an active mute group, set the member's fader node to 0; the member's own `mute` stays as it is, and the UI marks the mute button. `mute_safe` isolates a strip or bus from mute groups and from a VCA's mute (answer 2). | §9 "without changing their mute buttons' own state"; "mute groups (separate from VCAs), with safe/isolate per channel". |
| D88 | VCAs and mute groups only change fader values (ramped like any fader, with `ramp_ms` and a snapshot's crossfade): never a plan swap, also when members change. | Real time: moving a VCA must not rebuild the graph. |
| D89 | Soloing a VCA (PFL or AFL) solos all its members (a member's own solo kind wins); solo-in-place treats them as soloed. | §9 "VCA solo/PFL = PFL of all members". |
| D90 | Snapshots hold VCA fader and mute (section `vcas`, classes `fader` and `mute`); `recall_safe` on VCAs. | §9 "controllable from … snapshots … like any fader". |
| D91 | Spill is the client's view: the console shows only the VCA's strips and buses (through nested VCAs) until "Show all"; the `vca` widget shows them beside the VCA. | §9 "selecting a VCA shows its members on the console". |

### M6a: results (dev VM)

- `cargo test`: 105 unit tests (new: VCA offsets with nesting counted once, VCA
  and nested mutes, a VCA at −∞, mute groups with mute-safe, validation with
  cycles; the compiled fader values with the structure unchanged; VCAs in
  snapshots with sections and recall-safe).
- `tests/integration/vca.sh`, sample-exact through MXL: VCA at −20 dB on both
  members; nested −6 dB over −20 dB (0.05); VCA mute with the members' mutes
  unchanged; a cycle refused (400); PFL of a VCA on the monitor (its member,
  pre-fader); a mute group, a mute-safe strip, inactive again; a snapshot
  recalling the VCA (section `vcas`); no plan swap for any of it; a deleted strip
  leaving its VCAs.
- `ui.sh`: a VCA made and assigned in the panel, its fader by keys followed by the
  second client, spill, a mute group from the toolbar (the strip's own mute
  unchanged). `widgets.sh`: the `vca` widget framed.
- **Finding (again):** with the demo mixer running on the dev VM, the probes see
  late inputs even on other CPUs; the tests ran with the demo stopped (as noted in
  `docs/development.md`).
- **Finding (CI):** `processing.sh` failed once on its compressor check (peak
  −27.2 dBFS, ±0.3 around −27.5). The peak checks had no retry: a late input block
  fades the input and resets the envelope, a transient gets through. They now
  measure again when an input was late during the probe, like the sample checks.

### M6a: questions

| # | Question | Proposal |
| --- | --- | --- |
| 1 | Cap a member's total gain (fader + VCAs)? | No cap, as implemented; a console-like limit (e.g. +10 dB total) if wanted. |
| 2 | Does `mute_safe` also isolate from a VCA's mute? | No: mute-safe is the mute groups' isolate; a VCA's mute always mutes its members. |
| 3 | VCAs in layout pages (and so in the `fader-bank` widget)? | Not yet: the console shows every VCA in its own section. |
| 4 | Limits: 64 VCAs, 32 mute groups. | As implemented (§16 asks for 4 VCAs). |
| 5 | Merge M6a, then M6b? | Yes. |

### M6a: answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Cap a member's total gain? | Yes, at +15 dB; and every fader goes to +15 dB at most. |
| 2 | Does mute-safe isolate from a VCA's mute? | Yes: mute-safe protects always. |
| 3 | VCAs in layout pages and the `fader-bank` widget? | Yes. |
| 4 | Limits 64 VCAs, 32 mute groups. | Accepted. |
| 5 | Merge M6a, then M6b. | Yes. |

After the answers: faders of strips, buses and VCAs `-inf` … +15 dB (sends stay
at +10 dB, §5); a fader and its VCAs at most +15 dB (`MAX_FADER_DB`); `mute_safe`
also against a VCA's mute; layout pages and the `fader-bank` widget take VCAs (the
console and the widget show a VCA strip there); deleting a strip, bus or VCA takes
it out of layout pages (before, a layout page kept a deleted strip from being
deleted). The UI fader scale goes to +15 dB. `vca.sh` checks the cap (a fader at
+15 under two VCAs at +15: +15 dB, 5.62), +15.5 refused, a mute-safe strip under a
muted VCA, a VCA in a layout page and a deleted strip leaving it; `widgets.sh`
frames a `fader-bank` with a VCA.

### M6b: what exists

- `src/mix/model.rs`: `Bus.minus` (a mix-minus: `of`, `without`), `Mix::all_sends`
  (the sends and those mix-minus buses mirror), in the signal order and the loop
  check; `Strip::new`, `Bus::new`.
- `src/mix/operations.rs`: group, N-1, mix-minus, link, unlink;
  `src/operations_api.rs`: `POST /api/v1/operations/…`; `minus` in `PATCH
  /api/v1/buses/<id>`.
- `src/mix/compile.rs`: every bus sums its own and its mirrored sends.
- UI: `OperationsPanel.vue`, the mix-minus in the bus settings and on bus strips.
- `tests/integration/n1.sh`; `mxl-mixer-probe --correlate`; operations in
  `ui.mjs`.

### M6b: decisions

| # | Decision | Reason |
| --- | --- | --- |
| D92 | A mix-minus is a bus property, not copied sends: the bus takes every send into `of` (tap, level, mute, phase as they are) except those from `without`, besides its own sends (its own wins). It follows `of`'s sends, and a level change there is a value change, no plan swap. | §5 "mix-minus from bus: for any bus, generate 'bus minus X'"; an N-1 that does not follow the programme is wrong as soon as a fader moves. |
| D93 | N-1 from a bus (`of`): a mix-minus bus per participant (`n1-<strip>`, the bus minus that strip) or one of the selection (the bus minus all of it). Without `of`: a bus per participant with sends of its own from the selection's other strips (post-pan, 0 dB). Format: `of`'s, else mono. | §5 "N-1 bus per selected participant (each receives the sum of all others) or one N-1 of the selection; result is a normal signal" (question 1). |
| D94 | Group: a new bus (stereo unless asked); the strips' post-fader and post-pan sends move to it, it sends to each of those buses at 0 dB; pre-fader sends (auxes) stay. Refused while a mix-minus of one of those buses leaves out one of the strips: a sum cannot have one member taken out again. | §5 "selected strips → new subgroup bus with routing" (question 2). |
| D95 | Link makes one strip: mono strips (2, 3, 6, 8, 10 or 12) into stereo, 3.0, 5.1, 7.1, 5.1.4 or 7.1.4, their patches in order, the first strip's processing, fader and settings, a send to every bus any of them sent to (the first one's level); layouts, VCAs, mute groups, monitor sources, talkback returns and mix-minus exclusions move to it. Unlink splits a stereo strip into two mono strips (hard left and right) with copies of its processing, fader and sends. | §5 "stereo pair / surround link: link strips (shared processing and fader)": one multichannel strip shares them by construction, and its nodes link across channels (§6). |
| D96 | Operations make ordinary graph and answer what they made; nothing remembers the operation. | §1 "the operator sees a console; the engine sees a graph", "high-level operations generate the graph underneath". |

### M6b: results (dev VM)

- `cargo test`: 109 unit tests (new: group with the aux left alone and the refusal
  against a mix-minus, N-1 per participant and one, from a bus and from the
  selection, a mix-minus following its bus and its own send winning, loops through
  a mirror, link and unlink with their references).
- `tests/integration/n1.sh`, four tones into a mono programme:
  - N-1 per participant from the programme: each correlates 0.000 with its own tone
    and 0.577 with each other (three equal tones);
  - one N-1 of s1 and s2: 0.707 with tones 3 and 4, 0.000 with 1 and 2;
  - a mix-minus without s4 follows the programme: s2's send at −∞ there leaves it
    and the N-1s, no plan swap;
  - link of s3 and s4 into a stereo strip, and unlink: sample-exact on a stereo
    bus; the mix-minus's exclusion moved to the linked strip;
  - group of s1 and s2: refused while N-1s leave them out; then their programme
    sends go through the group.
- `ui.sh`: N-1s of Main for two strips from the Operations panel.
- **Finding (CI):** a correlation of the four-tone programme came out 0.40–0.48
  instead of 0.50 once (late input blocks during the measurement); the correlation
  checks now measure again when an input was late, like the sample checks, and a
  sum's tones count from 0.3 (a missing tone stays below 0.02).

### M6b: questions

| # | Question | Proposal |
| --- | --- | --- |
| 1 | N-1 "each receives the sum of all others": the bus it comes from minus the participant (it follows that bus), or only the selection's others? | Both, as implemented: with `of` the bus minus the participant, without `of` the selection's others. |
| 2 | A group of strips that a mix-minus leaves out is refused. Alternative: a mix-minus could also leave out a group's members (a group-minus underneath), at the cost of a bus per mix-minus. | Refuse, as implemented. |
| 3 | Link: the first strip's processing and fader, the union of the sends at the first one's level; unlink only for stereo. | As implemented. |
| 4 | Defaults: group stereo, N-1 the bus's format or mono, ids `group-<n>`, `n1-<strip>`, `minus-<n>`, `link-<n>`, `<id>-l`/`<id>-r`. | As implemented. |
| 5 | Merge M6b? Milestone 6 is then done; 1.0.0 is milestones 0–6 (§19): prepare it (release notes B.3, image digest, the maintainers' hardware tests)? | Yes, and prepare 1.0.0. |

### M6b: answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | N-1 from a bus and from the selection | Yes, both. |
| 2 | A group that would break a mix-minus is refused | OK; the maintainer tests it by hand (in the 1.0.0 checklist). |
| 3 | Link and unlink as implemented | Yes. |
| 4 | Defaults (formats, ids) | Yes. |
| 5 | Merge M6b, then prepare 1.0.0 | Yes. |

## Release 1.0.0 (preparation)

Milestones 0–6 are merged (§19: 1.0.0 = milestones 0–6, the widgets `strip`,
`fader-bank`, `loudness`, `monitor`, `talkback`, and tally/AFV; `vca` too). The
release PR sets the version to 1.0.0 (crate, UI) and adds:
- `docs/release-notes/1.0.0.md`: the GitHub release text of Appendix B.3 (image,
  every setting, ports, volumes, widgets, exit codes, what the platform must set,
  what is not in 1.0.0, what was tested); the digest and the hardware results are
  TODO until the tag and the maintainers' tests;
- `docs/release-notes/1.0.0-checklist.md`: the maintainers' hardware and manual
  tests, each with its pass criterion (including the group refused with a
  mix-minus, M6b answer 2).

**Performance at the release** (§19 "measured at every release";
`tests/perf/section16.sh`, now with 8 real N-1 buses, mix-minus of main without
one strip each, and 4 VCAs; release build, SCHED_OTHER, shared 4-vCPU VM, the
demo stopped): 1440 sends in the plan (672 and 768 mirrored), mean block 772 µs,
`dsp_load_ratio` 0.386 (< 0.5), every block ≤ 1.5 ms, 8 deadline misses in 60 s
(the 1 h, zero-miss target of §16 is for the platform node, in the checklist).

### Release 1.0.0: questions

| # | Question | Proposal |
| --- | --- | --- |
| 1 | Two items of §5 and §6 are not built: the read-write routing graph view (the matrix is there) and the oscillator's pink/white noise and sweep. In 1.0.0, or later? | Later (1.1.0), listed under "Not in 1.0.0". |
| 2 | Anything to announce to the platform's operators before the release (B.3: names, labels, ports, volumes)? | The release notes are the announcement; this is the first release. |
| 3 | When the checklist is done: may I tag `v1.0.0` on `main`, let `publish` build `1.0.0`, put the digest and the hardware results into the notes, and create the GitHub release? | Yes, after your results. |

### Release 1.0.0: answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | The routing graph view and the oscillator's noise and sweep: in 1.0.0 or later? | In 1.0.0. |
| 2 | The release notes as the announcement | Yes. |
| 3 | Tag and release after the checklist | The maintainer schedules the hardware tests; the tag waits for them. |

| # | Decision | Reason |
| --- | --- | --- |
| D97 | The oscillator's `white` and `pink` noise: independent on every channel (an xorshift generator per channel; pink with Paul Kellet's refined filter, scaled to the white noise's RMS), `level_dbfs` is the RMS; `sweep`: logarithmic from `freq_hz` to `sweep_to_hz` in `sweep_s` (0.1–600 s, defaults 20 kHz and 10 s), repeated, continuous phase, `level_dbfs` peak. Presets: pink and white −18 dBFS RMS, a 20 Hz–20 kHz sweep in 10 s. | §6 "oscillator/tone (sine, pink/white noise, sweep, EBU stereo ident)"; answer 1. Independent channels for speaker line-up (correlated noise would image in the centre); RMS for noise as in line-up practice (question 1). |
| D98 | The routing graph is a tab of its own with Vue Flow (MIT): inputs, strips, buses (in columns by routing depth), monitors and published outputs; edges for patches, sends (level, tap; pre-fader dashed, muted red), a mix-minus (one dotted edge from its bus), monitor sources and outputs. Read-write for sends: drawn from a strip or bus to a bus (0 dB from its output), edited in a panel from a click, deleted with Delete; deleting an output edge unpublishes. Patches, monitor sources and the mix-minus are shown, edited where they live. Moved nodes stay where they were put, per browser. | §5 "graph view: a visual routing graph (read-write) for advanced users; the console view stays the default"; §3 "Vue flow/graph library"; answer 1. The UI page grows from 227 to 439 kB (question 2). |

After the answers: both built, unit-tested (white and pink at −18 dBFS RMS within
0.2 dB, the channels uncorrelated, pink with equal energy per octave within 1 dB,
the sweep's level and its start and end) and in `room.sh` (pink noise at −18 dBFS
RMS on a published monitor with a correlation of 0.02 between its channels, a
sweep at −18 dBFS peak) and `ui.mjs` (a send drawn from a strip to a bus in the
graph, edited from its edge, removed with Delete).

**Finding:** Vue Flow checks every edge it is given with `is-valid-connection`,
not only new ones: the first version (any send needs a bus as its target) hid the
patch, monitor and output edges. The check now passes edges with an id.

### Release 1.0.0: questions after the answers

| # | Question | Proposal |
| --- | --- | --- |
| 1 | Noise: independent channels and the level as RMS. | As implemented. |
| 2 | The UI page is 439 kB (141 kB compressed) with Vue Flow. | Fine for an embedded UI. |

## M7a: voice activity detection, auto-mix, ducking

Milestone 7 (§19, release 1.1.0) in two parts, each a PR:
- **M7a** (PR 17, on the release branch until `v1.0.0` is tagged, then into
  `main`): VAD (§8.1), the gain-sharing auto-mixer (§8.3), ducking by speech
  (§8.4).
- **M7b**: the speech-aware auto-level (§8.2) with Com-IS-Assist's core (MIT, its
  notice kept), the master_me / PodcastPlugins comparison, chain presets, the
  simple view and the loudness normaliser for outputs (D40), as planned above;
  and a duck released by speech in its own target (Com-IS-Assist's interview
  passthrough, answer 12).

### M7a: what exists

- `src/mix/auto.rs`: `vad` (per-signal settings), `automixes`, `ducks` in the mix;
  the analysed signals (own settings, auto-mix members, duck keys), the signals
  with an auto gain (members, targets), validation; references follow deletes,
  links and unlinks.
- `src/engine`: 64 VAD tap slots, each a ring of 0.5 s allocated with the engine;
  `Step::Vad` writes the mean of a signal's channels before its fader.
- `src/mix/compile.rs`: the auto gain node (`strip/<id>/auto`, `bus/<id>/auto`)
  after the fader (after AFV; on a bus before talkback), the VAD tap.
- `src/auto/`: `vad.rs` (the detector, the `Detector` trait, hysteresis, hold,
  outputs, an evaluation tool), `automix.rs`, `duck.rs`, `mod.rs` (slots, the
  10 ms tick, the gains); `Control::auto_tick`.
- `src/auto_api.rs`: `/api/v1/vad`, `/api/v1/automixes`, `/api/v1/ducks`; the
  VAD state and auto gains in the meters message; `vad_active{signal}`; `auto`
  in the status.
- `src/wav.rs`, `mxl-mixer-tone --wav --gap-s --offset-s`.
- UI: `AutomationPanel.vue` (console toolbar "Auto"), the voice dot and the auto
  gain on strips.
- `tests/integration/auto.sh`; `ui.mjs` (auto-mix and duck from the panel);
  `scripts/demo.sh` (two synthetic voices in turns, an auto-mix, the music
  ducked).

### M7a: decisions

| # | Decision | Reason |
| --- | --- | --- |
| D99 | The VAD runs on the control side on a tap before the fader: the DSP thread copies the mean of the signal's channels into one of 64 rings (0.5 s each, allocated with the engine, never by the DSP thread); a 10 ms tick takes the rings, runs the VADs and the controllers, and sends changed gains, ramped over 10 ms. A slot given to another signal drops what its ring holds once the DSP thread runs the new plan. | §8.1 "runs on the control side from tapped audio"; §2 real time. Before the fader, so the operator's fader and the auto gain do not change what is detected (an auto-mix would otherwise follow its own gain). The mean keeps a centred stereo talker at its level. |
| D100 | v1's detector, per 10 ms frame: the 250 Hz–4 kHz band's power over a minimum-statistics noise floor (smoothed, eight 250 ms windows); voicing (the largest normalised autocorrelation for 80–400 Hz on that band low-passed at 1 kHz and decimated to 6 kHz, 40 ms window); modulation (standard deviation of the band power over 400 ms); pure-tone rejection (an autocorrelation reaching −0.93). Probability = energy × modulation × (0.3 + 0.7 × voicing), each a smoothstep between two values; voicing and the tone count only when the voicing window's halves are within 10 dB. Behind a `Detector` trait. | §8.1 "lightweight, CPU-cheap (spectral and energy features with hysteresis); the interface allows a model-based detector later". Tones (line-up, idents, beeps, sweeps) are a broadcast mixer's daily signals and must never be speech. 0.09 % of a core per analysed signal (64: 6 %). The feature values were set on synthetic speech, noise and tones, and checked on espeak-ng speech and LibriVox readings (question 8). |
| D101 | Speech starts when the smoothed probability (rising 0.5, falling 0.2 per frame) reaches `threshold` and ends `hold_ms` after it fell below `threshold` − 0.15; nothing below `min_level_dbfs` is speech. Outputs: `speech_active`, `speech_probability`, `speech_level_dbfs` (power mean of the speech frames, 1 s), `level_dbfs`, onset and offset counts, the features. Detection latency, measured: a voiced onset in 30 ms (median), 40 ms (90 %), 100 ms at most (words starting with a fricative); the tick adds up to 10 ms. | §8.1 outputs and "documented detection latency". Counts for onsets and offsets so that the logic (M8) can trigger on their change. |
| D102 | One auto gain node per signal after its fader (strip: after AFV, before the panner; bus: before talkback), present while the signal is an auto-mix member or a duck target: the auto-mix gain × the deepest duck. Changing a gain never swaps the plan; adding or removing a member or target does. | §8.3 "a controller producing gains for the matrix/fader nodes, so operator fader moves combine with it (auto-mix gain × operator gain)"; §8.2 "drives a gain node, not hard-coded DSP". As AFV does. Pre-fader sends do not get it (question 2). |
| D103 | Gain sharing: g_i = √(w_i L_i / Σ w_j L_j); L the frame's mean square, rising at once, falling with 100 ms; w = 10^(weight_db/10), with VAD weighting × (p + 0.1 (1 − p)). NOM attenuation is the sharing itself (N equal members: −10·log10 N). Bypassed members: 0 dB, out; muted members (mute, VCA, mute group, fader at −∞): out, gain kept; a member shares from its first measured frame. Last-mic-hold: while no sharing member's VAD reports speech, the gains stay. | §8.3 "each channel's gain follows its share of the total level, with last-mic-hold, NOM attenuation, per-channel weight and bypass; optional VAD weighting". The powers add up to one open microphone, as in Dugan's design (questions 3–5). |
| D104 | Ducking: while a key speaks (its VAD), the targets go linearly in dB to `depth_db` in `attack_ms`, stay `hold_ms` after the last speech, come back in `release_ms`; −∞ ramps as −120 dB. A target of several ducks takes the deepest; a key is never its own target. Speech priority is a duck from a presenter to the guests. | §8.4 "duck any signal under speech detected on other signals, with depth, attack, hold, release". Linear in dB over a time as AFV's fades; the DSP ducker (D29) keeps its time constants. |
| D105 | VAD settings, auto-mixes and ducks are in the mix (show file, history), not in snapshots. In the API a bare member id keeps that member's weight and bypass, an object sets them. Limits: 64 analysed signals, 16 auto-mixes, 32 ducks. | Snapshots: question 6. Bare ids let a UI add and remove members without losing their settings. |
| D106 | Test speech is synthesised at test time with espeak-ng (CI installs it) and played into MXL by `mxl-mixer-tone --wav`; no recording is committed. Real recordings (LibriVox, public domain) were used only locally, with the evaluation tool (`docs/development.md`). | Licence-clean and reproducible; the maintainers' own recordings are the better check (question 7). |

### M7a: results (dev VM)

- `cargo test`: 126 unit tests (new: the VAD on silence, pink noise, sines of
  200 Hz, 1 kHz and 3 kHz, beeps and a 20 Hz–20 kHz sweep (never speech), on
  synthetic speech (every word detected, ≤ 60 ms, > 95 % of the speech frames,
  > 99 % of the pauses free) and on speech in pink noise 15 dB lower; hold,
  threshold, level; gain sharing (NOM, a talker, weights, bypass, mutes,
  disabled, the 100 ms fall, last-mic-hold, a new member, VAD weighting); duck
  ballistics; slots; the compiled auto node and tap; the WAV reader; the model).
- `tests/integration/auto.sh` through MXL:
  - two equal tones in an auto-mix: 0.70710677 each (−3.01 dB), sample-exact; a
    weight of +6 dB: 0.89400 and 0.44806; bypass: both 0 dB; a muted member out
    of the sharing; disabled: 0 dB; none of it swapped the plan;
  - the VAD on espeak-ng speech: one onset per 6 s period, speech level
    −22.5 dBFS; on a 1 kHz tone: none;
  - a tone ducked under the speech by −20 dB: 75–85 ms from the speech onset to
    −10 dB (detection, the tick, half the 50 ms attack; four periods in three
    runs), 99.5–100 % of the speech at −20 ± 0.5 dB, back to 0 dB 1.5 s after it;
  - two talkers in turns with last-mic-hold: the one speaking above −1 dB and
    the other below −20 dB in 100 % of the polls; with nobody speaking, the last
    talker held.
- `ui.sh`: an auto-mix and a duck made in the Auto panel; the strips show the
  shares (their powers add up to 1) on both clients, a weight moves them.
- All 15 integration suites pass on release binaries (the demo stopped; `tally.sh`
  once only on the second try, its input late on this VM).

**Findings:**
- A member added to an auto-mix got 0 and last-mic-hold kept it there: its tap
  starts with the next plan, and before it, a missing level counted as silence.
  A member now shares from its first measured frame.
- The UI built member lists from the last pushed mix; two quick clicks lost the
  first. The panel's changes now go through the store's local changes (as
  faders do).
- With the processing of the UI test (an EQ boost near Vox's tone) Vox gets the
  larger share: the sharing reads after the chain, as intended.
- The Auto panel read the live VAD state, so the meters (25 Hz) re-rendered it,
  and Vue set every input back to its value: typing a weight was undone within
  40 ms (CI failed once on it). The live readouts are a component of their own
  (`AutoLive.vue`), and the numbers are fields that keep what is typed while
  they have the focus (`NumberField.vue`, also against a mix pushed meanwhile);
  `ui.mjs` types at a person's pace.

### M7a: questions

| # | Question | Proposal |
| --- | --- | --- |
| 1 | Defaults and limits: VAD threshold 0.5, hold 300 ms, minimum level −60 dBFS; duck depth −12 dB, attack 50 ms, hold 500 ms, release 1000 ms; weights −30 … +10 dB; 64 analysed signals, 16 auto-mixes, 32 ducks. | As implemented. |
| 2 | The auto gain sits after the fader: pre-fader sends (auxes, IFBs) are neither auto-mixed nor ducked. Before the fader (like an insert) they would be. | After the fader, as implemented: foldback stays as the operator set it. |
| 3 | A muted member (also a fader at −∞) leaves the sharing. | As implemented. |
| 4 | NOM attenuation is the sharing itself, no switch of its own. | As implemented. |
| 5 | Last-mic-hold uses the members' VAD (hold while nobody speaks). | As implemented. |
| 6 | Auto-mixes and ducks in snapshots (on/off, weights, depths)? | Not now; a section `auto` later if wanted. |
| 7 | A 1.1.0 checklist with real microphones on the maintainers' productions (the VAD's evaluation tool on recordings, crosstalk between panel microphones, ducking music under a presenter). | Yes, with 1.1.0. |
| 8 | Music on an analysed signal often counts as speech (voiced and modulated); v1 is meant for microphones. | A known limit of v1; a model-based detector behind the same trait later. |
| 9 | Merge M7a into `main` after `v1.0.0` is tagged, then M7b. | Yes. |

### M7a: answers (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Defaults and limits | Yes. |
| 2 | The auto gain after the fader? | No: configurable like the processing chain; before the fader by default. |
| 3–5 | Muted members leave the sharing; NOM without a switch; last-mic-hold by the VAD | OK. |
| 6 | Auto-mixes and ducks in snapshots? | Needed: e.g. first a moderation snapshot, then a panel discussion. |
| 7 | A 1.1.0 checklist with real microphones | With recordings of productions. |
| 8 | Music as a known limit of v1 | Not asked on its own; see question 10 (Silero). |
| 9 | Merge 7a after `v1.0.0`, then M7b | Yes. The release PR #16 is merged (approved the same day). |

And: learn from Com-IS-Assist's voice activity detection, which its author
tested on many recordings.

### M7a: what Com-IS-Assist teaches

Com-IS-Assist (Andy Weiss, MIT; `crates/com-is-assist-core`,
`Specs/TechnicalConcept.md` §5–6) uses **Silero VAD** (a small recurrent
network, MIT, vendored as ONNX, run with ONNX Runtime through `ort`, 16 kHz,
32 ms chunks, threshold 0.5) and records what its tests on commentary showed:
- **The hold must span a pause between sentences.** With 300 ms (our first
  default) every ~1.2 s pause of real commentary dropped "voice" and the bed
  swelled up at sentence rate. Its defaults: 0.8 s VAD hangover and a 0.8 s
  release hold: the bed recovers after ~1.6 s of quiet.
- **A level gate against crosstalk.** "A detector answers *is this speech?*,
  not *is this speech on this microphone*": the programme bleeding into a
  commentary microphone is speech, and the bed ducked against its own bleed.
  It ignores voice while the microphone's momentary loudness (400 ms) is below
  a gate, engaged by default at −32 LUFS (measured: commentary median
  −26 LUFS; a −40 LUFS gate removes ~10 % of detections, the quiet tails and
  spill). Set 20–30 dB below the talker.
- The bed's VAD reads the dry bed, before the automix gain (no second feedback
  path): as our tap at the auto gain's input.
- Bed speech while the commentator is silent (an interview in the bed) releases
  the ducking faster ("interview passthrough"): an idea for M7b, with
  Com-IS-Assist's ratio control.

**Measured here** (the VAD's evaluation, a 110 s sequence of LibriVox speech,
Chopin piano and Brahms orchestra from Musopen (CC0), pink noise, speech over
piano 12 dB down, a tone and silence; fraction of 10 ms frames with speech):

| Segment | ours (hold 0.3 s) | Silero (0.3 s) | Silero (0.8 s) |
| --- | --- | --- | --- |
| speech | 0.75 | 0.78 | 0.88 |
| piano | 0.53 | 0.00 | 0.01 |
| orchestra | 0.16 | 0.00 | 0.00 |
| pink noise −30 dBFS | 0.00 | 0.00 | 0.00 |
| speech over piano | 0.63 | 0.81 | 0.87 |
| piano | 0.30 | 0.07 | 0.11 |
| tone, silence | 0.00 | 0.00 | 0.00 |

On clean speech both agree (95 % of Silero's speech frames are ours); on music
ours fires, Silero does not; under a music bed ours misses a quarter of the
speech. Silero: 0.08 ms per 32 ms chunk with ONNX Runtime (0.26 % of a core per
signal; 64 signals ~17 %). A pure-Rust engine (`tract` 0.21 and 0.23) cannot
run the model (its `If` nodes for 8/16 kHz; folding them with onnxsim failed
on the nested decoder `If`); ONNX Runtime through `ort`, as Com-IS-Assist,
works (question 10).

### M7a: decisions after the answers

| # | Decision | Reason |
| --- | --- | --- |
| D102 (revised) | The auto gain is a chain node (`{"type": "auto"}`, one per chain, placed like any node; bypass: no auto gain); without one it acts after the chain, before the fader. Pre-fader sends and PFL carry it; the VAD reads at its input, so a controller never hears its own gain. | Answer 2. |
| D101 (revised) | VAD `hold_ms` 800 (was 300), duck `hold_ms` 800 (was 500); `min_level_dbfs` (a 10 ms frame's RMS, effectively open at −60) is replaced by `gate_lufs` (−32): a frame counts as no speech while the signal's momentary loudness (K-weighted, 400 ms, BS.1770) is below it; the state shows `momentary_lufs` and `gated`. The gate delays a detection after silence by 400 ms × 10^((gate − level)/10): voiced onsets now 50 ms (median), 70 ms (90 %), 100 ms at most; ducking 81 ms to −10 dB in `auto.sh`. | Com-IS-Assist's measurements (above): sentence pauses and crosstalk. The gate acts on frames, so the hold still runs from the last speech above it. |
| D105 (revised) | Snapshots hold the auto setup as a whole (`auto`: VAD settings, auto-mixes, ducks); section and class `auto`; a signal filter does not narrow it; recall-safe auto-mixes and ducks stay (and keep their strips); signals deleted since are dropped; snapshots without `auto` (older) leave the setup. | Answer 6: a moderation, then a panel discussion, is a whole setup. |
| D109 | A 1.1.0 checklist on recordings of productions (`docs/release-notes/1.1.0-checklist.md`): the evaluation tool per track, multitrack playback with `mxl-mixer-tone --wav` for the auto-mix, the gate and ducking, snapshots, live microphones and the load. | Answer 7. |

Results: 127 unit tests (new: the auto node before the fader, a pre-fader send
carrying it, an explicit node moving the gain and the tap, one per chain;
snapshots with the auto setup: moderation and panel in turn, a strips-only
recall, recall-safe, a deleted strip, an older snapshot; the gate; the 80-frame
hold). `auto.sh`: the auto gain before the fader (a pre-fader aux at 0.707, the
fader on top only in the programme), snapshots switching moderation and panel;
`ui`, `show`, `mix`, `processing`, `widgets`, `vca` unchanged.

### M7a: questions after the answers

| # | Question | Proposal |
| --- | --- | --- |
| 10 | Silero VAD as the detector (default), ours kept as the fallback (and for builds without it)? It adds ONNX Runtime (Microsoft, MIT, C++, linked statically; `ort`'s build downloads its prebuilt library once per build machine, also in CI and the image build) and the 2.3 MB model (MIT, vendored as in Com-IS-Assist); 0.26 % of a core per analysed signal. | Yes: the measurements above, and the detector Com-IS-Assist was tuned with. |
| 11 | The gate at −32 LUFS by default (Com-IS-Assist's), set per microphone 20–30 dB below its talker. | As implemented; the checklist notes the values that work. |
| 12 | Bed speech releasing a duck (Com-IS-Assist's interview passthrough). | With M7b (Com-IS-Assist's ratio control). |

### M7a: answers 10–12 (maintainer, 2026-10-12)

| # | Question | Answer |
| --- | --- | --- |
| 10 | Silero VAD as the default detector | Yes. |
| 11 | The gate at −32 LUFS | To be tried (the 1.1.0 checklist). |
| 12 | Bed speech releasing a duck (interview passthrough) | With M7b, as proposed. |

| # | Decision | Reason |
| --- | --- | --- |
| D110 | Silero VAD (`assets/silero_vad.onnx`, upstream `bfdc019`, sha256 `1a153a22…`, unchanged, MIT; the file Com-IS-Assist vendors) is the default detector (`detector: "silero"`), the feature detector stays (`"features"`, and the fallback when ONNX Runtime cannot load the model: a warning, `state.detector` tells). The 48 kHz frames are decimated by 3 (a 95-tap Blackman windowed sinc at 7.2 kHz: −60 dB from 9 kHz) into 512-sample chunks with 64 samples of context and the recurrent state per signal; a frame's probability is the last chunk's; the smoothing, threshold, gate and hold are the VAD's, as for the feature detector. One ONNX Runtime session for every signal, one intra- and one inter-op thread: inference runs on the control side's thread, never as a thread pool next to the DSP thread. ONNX Runtime 1.31 through `ort` 2.0.0-rc.14 (MIT OR Apache-2.0), linked statically; `download-binaries` fetches the prebuilt library once per build machine (CI and the image build too). The image carries ONNX Runtime's licence and third-party notices (Eigen MPL-2.0 among them) and Silero's licence. | Answer 10; the comparison above. |

Results (dev VM): Silero in the mixer, on the same 110 s sequence with the
defaults (hold 0.8 s, gate −32 LUFS): speech 0.86 (features 0.85), piano 0.00
(0.69), orchestra 0.00 (0.23), speech over piano 0.86 (0.79), the second piano
piece 0.13 (0.46), noise, tone, silence 0.00. Cost: 0.83 % of a core per
analysed signal (one thread; 64 signals about half a core). On the synthetic
words of the unit tests (formant tones, not speech) it misses some and is late:
those tests stay with the feature detector; Silero's own tests: silence, pink
noise and tones never above 0.5, the decimator (±0.1 dB at 1 kHz, below −60 dB
from 9 kHz). `auto.sh` with Silero as the default: the espeak speech detected
once per period, the tone never, ducking 81–91 ms from the onset to −10 dB,
the two talkers 98–100 %; `ui.sh` passes. The binary now needs `libstdc++`
(installed in the image, checked with `ldd`).

### Release 1.0.0: the maintainers' hardware tests (2026-10-13)

On `f31a5dd` (2× Xeon Gold 6136, kernel 6.8 with voluntary preemption, no
isolated CPUs, other real-time load on the host; the runner on 9 CPUs, the DSP
thread at SCHED_FIFO 49): env, the 10-min block clock, the platform registry,
AMWA and the API checks pass; **the §16 load fails** (`dsp_load_ratio` 0.854,
mean block 1708 µs, 237 432 deadline misses in 60 min; 0.851 again on a quieter
host). Their profile of the DSP thread: true peak 27 %, `pow` 17 % and `log10`
11 % (compressors, de-essers), biquads 12 %, PPM 6 %. The 30- and 60-min block
clock runs missed 3 and 6 deadlines (up to 2.9 ms): the host, not the mixer. The
tag waits for the load re-run.

| # | Decision | Reason |
| --- | --- | --- |
| D111 | The signal meters (peak, true peak, PPM, energy, correlation) move to a meter thread. The DSP thread's `Meter` step writes each block as a record (plan generation, slot, channels, length, samples) into one engine-lifetime ring of 2^22 values (half a second of 160 channels); a record that does not fit is dropped and counted (`meter_drops_total`). The meter thread (FTZ/DAZ, off the DSP thread's CPU when there are others) takes the ring every 2 ms and writes the plan's `MeterBank` atomics, which the control side reads as before; the control side gives it a plan's bank before the plan is sent. Loudness stays on the DSP thread. | About a third of the DSP thread's time with every signal metered; §7 asks for the meters at 25 Hz, not on the real-time thread (the analyser was already on the control side). Loudness must not lose a block. |
| D112 | dB conversions without libm: `log2` from the exponent bits and the series 2·atanh to z⁹ on the mantissa in [1/√2, √2), `exp2` by the nearest integer into the exponent and the Taylor series to t⁸ (errors below 1e-8 dB and 1e-9 relative, tested against libm), with float tricks instead of integer conversions so that they vectorise. The compressor and the de-esser run in passes over the block (level, dB, curve, ballistics, gain), only the ballistics sample by sample; detection still reads every sample before its gain, so the result is that of per sample. The compressor's soft knee is computed with clamp and max. | `pow` and `log10` per sample were 28 % on their host; a block's conversions vectorise when they do not wait on the envelope. |
| D113 | True peak: the four phases at once per sample (one 4-wide multiply-add per tap), the same sums in the same order (bit-identical to the first version, tested); a block whose largest |sample| (with the 11 before it) × the interpolator's largest Σ|h| cannot exceed the peak held since the last meter tick is not oversampled. | 3.6× faster per channel; the skip is exact (and helps on quiet passages, not on a steady tone). |
| D114 | Biquad chains (EQ, filters, the de-esser's split) run every sample through all their sections in turn instead of section after section over the block: the same arithmetic, the sections' recursions overlap in the CPU. Peaks and sums of squares over a block are reduced in eight lanes. | The EQ 1.9× faster; a fold with `f32::max` or an f64 sum is one dependency chain the compiler may not reorder. |
| D115 | The build stays generic x86-64; `target-cpu=x86-64-v3` (the platform's minimum) gained 10–20 % in the compressor and true peak alone but under 2 % on the §16 load. | Not worth a binary that refuses older CPUs, now. |

**Measured** (development VM, release build, SCHED_OTHER, the demo stopped;
`cargo test --release --lib dsp_costs -- --ignored --nocapture` for the nodes):

| Per 96-sample block | Before | After |
| --- | --- | --- |
| compressor (peak / RMS) | 2801 / 2916 ns | 1130 / 1120 ns |
| de-esser (split-band) | 2707 ns | 1780 ns |
| EQ, 4 bands | 1043 ns | 550 ns |
| true peak, one channel | 1440 ns | 400 ns (on the meter thread) |
| PPM, both types | 393 ns | 380 ns (on the meter thread) |
| §16 load: mean block, `dsp_load_ratio` | 755 µs, 0.378 | 298 µs, 0.149 (twice), every block ≤ 1.5 ms, 0 missing samples, 0 meter drops |

Scaled by their host's ratio to this VM (0.854 / 0.378), the §16 load would be
about 0.34 there.

**Their test-script points**, fixed: `results.md`'s `amwa` row is text ("IS-04-01:
Pass 39, …; …"), and a `|` in any row is escaped; the host name is `NODE_NAME`
when set (the Kubernetes downward API), and in a container the report says it
is one; `ci/amwa.sh` removes its `/dev/shm/mixer-amwa.*` folders and takes the
registry's ten ports from `AMWA_REGISTRY_PORT` (default 8010), failing with a
clear message when one is taken. (Its first failure without internet access:
nmos-testing clones the AMWA specifications; the runner's README says so.)

### Release 1.0.0: questions after the hardware tests

| # | Question | Proposal |
| --- | --- | --- |
| 1 | Re-run `LOAD_MINUTES=60 tests/hw/release/run.sh load` and the 10-min block clock on this branch (`nightly-dev` after the merge; note its `git-<sha7>`), on a production-like node if there is one. | Yes: the tag goes on that commit after a pass. |
| 2 | The block clock on a node with a fully preemptible kernel and isolated CPUs, for a verdict on the misses. | When such a node is available; not a blocker for 1.0.0 (every sample was correct). |
| 3 | Merge this PR into `main` (and then into milestone 7a's branch). | Yes. |

## M7b: auto-level and ratio ducks (Com-IS-Assist's control)

Milestone 7, part 2 (PR 19). `v1.0.0` is tagged and M7a is in `main`. This part:
the speech-aware auto-level
(§8.2) and Com-IS-Assist's ratio control as a duck mode, with the interview
passthrough (M7a answer 12). Still to come in M7b (a PR of its own): the
master_me / PodcastPlugins comparison, chain presets, the simple view and the
loudness normaliser for outputs (D40).

### M7b: what exists

- `src/auto/comis/`: Com-IS-Assist's control core, copied (below).
- The VAD taps carry every channel: `Step::Vad` writes each block as a record
  into one ring (`engine::meter::write`/`read`, as the meters);
  `Auto::planned(generation)` makes a reassigned slot's records count from the
  plan that has it.
- `src/auto/level.rs`: BS.1770 loudness per tapped signal on the control side.
- `src/auto/autolevel.rs`, `src/auto/ratio.rs`: the controllers; `src/auto/mod.rs`
  cuts the records into frames and combines the gains per channel.
- `src/mix/auto.rs`: `autolevels`, a duck's `mode`, `ratio`, `channel_depth`;
  validation, references, snapshots (`auto` section).
- `src/auto_api.rs`: `/api/v1/autolevels`, the ducks' new fields (partial
  updates of `ratio` and `channel_depth`) and `control`; the meters message's
  `autolevels` and `ducks`; `vad_drops_total`; the status's `autolevels`.
- UI: the Auto panel's auto-level table, a duck's mode, ratio settings, live
  control and channel shares.
- `tests/integration/auto.sh` (auto-level, ratio duck), `ui.mjs`, the demo.

### M7b: decisions

| # | Decision | Reason |
| --- | --- | --- |
| D116 | Com-IS-Assist's control core is copied, not depended on: `mix_state.rs`, `envelope_detector.rs`, `loop_bank.rs`, `automix_engine.rs` and their tests from `crates/com-is-assist-core` at commit `5bdd96b` into `src/auto/comis/`, each with a header (source, copyright, MIT, what changed) and its `LICENSE`, in the image under `/usr/share/doc/com-is-assist`. Changed: the silence sentinel is the mixer's (−100 LUFS, no `ebur128`), `DuckDepth::per_channel` maps channel labels (L/R front, C/M centre, LFE, everything else surround); formatted with the mixer's rustfmt. Not copied: its `ebur128` meters, its Silero wrapper and its VST3/offline wrappers (the mixer has its own). | §8.2 "integrate the following function from this repo"; MIT allows it with the notice. The crate is not published and pulls `ebur128` and its own ONNX wrapper. |
| D117 | The VAD taps are records in one ring of 2^22 values (0.68 s of 64 stereo signals) with the plan generation, instead of 64 mono rings of the channels' mean; a record that does not fit is counted (`vad_drops_total`). A slot given to another signal counts records from the plan that has it (`Auto::planned`). | A bed's loudness needs every channel with its weight (BS.1770); the generation replaces the plan-swap counter the mono rings needed, and the shared ring holds wider signals without 64 large rings. |
| D118 | Each tapped signal's loudness on the control side, per 10 ms frame: K-weighting per channel, the BS.1770 channel weights by label, momentary (40 frames) and short-term (300 frames), and a fast exponential mean square (50 ms, Com-IS-Assist's `FAST_WINDOW_SECONDS`) on the same scale. The VAD's gate reads this momentary loudness. Cost: 3.2 µs per 10 ms frame of a stereo signal (0.03 % of a core; 64 signals 2 %). | One measurement for the gate, the auto-level and the ratio ducks. For a stereo signal the gate now reads the BS.1770 sum (a centred talker 3 dB above the mean the gate read in M7a; question 5). |
| D119 | Auto-level, per frame: while the momentary loudness is below `freeze_lufs` the gain holds and nothing is measured; entering the freeze takes the last 400 ms (the pause that led to it) back out. The level is the mean power of the last 300 frames with signal (short-term loudness over the signal, pauses under 400 ms included); the target is `target_lufs` + `speech_offset_lu` while the VAD speaks; the gain goes to target − level within the range, a one-pole in dB (`attack_ms` down, `release_ms` up). Defaults: −23 LUFS, +2 LU (§8.2's examples), ±12 dB, 1 s / 3 s, −50 LUFS (question 1). Feed-forward: it reads before its own gain. | §8.2: "a loudness target (e.g. −23 LUFS short-term) and a configurable speech offset (e.g. +2 LU) while speech is active; attack/release and max gain range; freeze when no signal". Without taking the pause back out, every pause would pull the level down by up to 400 ms of silence. |
| D120 | A ratio duck (`mode: "ratio"`) runs one Com-IS-Assist `AutomixEngine` per duck: COM voice = a key's VAD (with its gate, Com-IS-Assist's guard band), IS voice = a target's VAD (`bed_voice`, default on); dialogue = the keys' short-term loudness, bed = the targets' short-term, momentary and fast loudness, powers added over several keys or targets, each with its auto-level's gain; each 10 ms frame steps the slow/mid stages and the fast stage once (Com-IS-Assist: 100 ms and 64 samples; its stages are one-pole filters over the elapsed time, so only the fast stage's resolution changes, and the gain ramps over 10 ms anyway). Settings and ranges are its plugin's: ratio 5 LU (0–12), over-voice 13 (0–24), at most 24 dB (0–48), speed 1 (0.25–8). The engine's ballistics, ease-in and interview speed-up are its own, unchanged. | Answer 12 ("interview passthrough with Com-IS-Assist's ratio control"); its author tuned these on commentary. A duck mode, so keys, targets, snapshots and the UI are those of ducks. |
| D121 | Channel depth groups (`channel_depth`: front, centre, LFE, surround; 0–1 of the dB) for fixed and ratio ducks; the auto gain is per channel: auto-level × auto-mix × each channel's deepest duck. The meters message's `auto_db` is the deepest channel. | Com-IS-Assist's duck depth: a surround bed keeps its atmosphere and LFE while the front ducks. |
| D122 | An auto-mix shares the members' levels with their auto-level's gain (power × gain²). | Gain sharing is about the levels as they reach the mix; an auto-levelled quiet microphone would otherwise get too small a share. |
| D123 | Auto-levels are in snapshots (the `auto` section as a whole; recall-safe ones stay); `PUT /api/v1/autolevels/<signal>` sets the fields given; a duck's `ratio` and `channel_depth` take partial objects. | As the VAD settings and ducks (D105). |

### M7b: results (dev VM)

- `cargo test`: 171 unit tests (new: loudness against EBU Tech 3341 case 1
  (−23.0 LUFS), the LFE and surround weights, the windows and the fast
  detector; the auto-level's target, speech offset, range, ballistics, freeze
  and a pause taken out; the ratio control's target (8 dB for 3 LU under),
  over-voice (16 dB), ceiling, ease-in, interview (more than 3× faster back),
  bed voice off, silence, disabled, speed; a 5.1 bed under synthetic speech
  with the LFE's share 0 and the surrounds' 0.5: front and centre at the
  reduction, surrounds at half, LFE at 0 dB; tap records and plan generations;
  the model, references, snapshots, older ducks reading as fixed).
- `tests/integration/auto.sh` through MXL, four runs in a row:
  - auto-level: a 1 kHz tone at −23.0 LUFS to a target of −30: gain −7.0 dB,
    the programme at −30.01 dBFS RMS; on speech (espeak, 6 s periods) the
    target −21 in the speech polls, the gain unchanged in the frozen ones;
  - a ratio duck of the commentator over a bed of a tone and a guest speaking
    in his pauses: every state seen; while ducking the median ratio 5.9–6.0 LU
    (reductions 4.9–12 dB); at the end of each of the guest's turns
    0.03–0.1 dB (interview passthrough); a partial `ratio` patch; a ratio of
    13 LU refused.
- `ui.sh`: an auto-level from the panel (Guitar to −30 LUFS, the gain the
  controller reports shown on its strip); the duck switched to ratio, 8 LU,
  the LFE's share 0.
- The demo: the music ducks to a ratio of 5 LU under host and guest (measured
  5.6 LU, 10.4 dB), both auto-levelled.

**Findings:**
- The ratio held while ducking is about 1 LU over the target on the test's
  speech: after each 3.7 s pause the commentator's short-term loudness builds up
  again from silence (the first frames ask for the most), and the bed's window
  still holds the guest's last turn. Com-IS-Assist's commentary pauses are
  about 1.2 s; on such speech the 3 s window does not empty.
- A signal tapped from a new plan needs 3 s for its short-term loudness; the
  ratio test waits for it.

### M7b: questions

| # | Question | Proposal |
| --- | --- | --- |
| 1 | Auto-level defaults: range ±12 dB, attack 1 s (down) and release 3 s (up), freeze below −50 LUFS momentary (target −23 and offset +2 are §8.2's examples). | As implemented; the 1.1.0 checklist tries them on productions. |
| 2 | The auto-level measures short-term loudness over the frames with signal (pauses under 400 ms count, longer ones not). Or plain short-term (every pause pulls the level down, then the gain up), or gated like integrated loudness? | As implemented. |
| 3 | Ratio ducks: Com-IS-Assist's defaults and ranges unchanged (5 / 13 LU, 24 dB, speed 1), its ballistics and ease-in fixed as there. | As implemented. |
| 4 | Several keys count as one dialogue and several targets as one bed (powers added); a mono target takes the centre group's share. | As implemented. |
| 5 | The VAD's gate now reads BS.1770 loudness over a signal's channels: for a stereo microphone or a stereo bed that is up to 3 dB more than M7a's mean of the channels. | As implemented (it is what LUFS means); the checklist notes gate values per microphone. |
| 6 | The 1.1.0 checklist now has the ratio duck (a commentary over a bed with an interview, a 5.1 bed with the LFE held) and the auto-level (a quiet and a loud guest, with the auto-mix). Anything to add? | As written. |
| 7 | Merge order: M7a after `v1.0.0`, then this PR, then the rest of M7b (comparison, presets, simple view, output normaliser). | Yes. |

### M7b: answers (maintainer, 2026-10-13)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Auto-level defaults (±12 dB, 1 s / 3 s, freeze below −50 LUFS) | Yes. |
| 2 | Short-term loudness over the frames with signal | OK. |
| 4 | Several keys as one dialogue, several targets as one bed; a mono target takes the centre share | Yes. |
| 5 | The VAD's gate on BS.1770 loudness over a signal's channels | Yes. |
| 6 | Anything to add to the 1.1.0 checklist? | No. |
| 7 | Merge order | As answered before (M7a answer 9): M7a after `v1.0.0`, then M7b. |

Question 3 (Com-IS-Assist's ratio defaults and ballistics unchanged) stays open;
they stay as implemented until then.

### Release 1.0.0: the maintainers' re-run (2026-10-13)

On `208689e`, the same host (2× Xeon Gold 6136, kernel 6.8 with voluntary
preemption, the DSP CPU's SMT sibling outside the runner and busy with another
tenant's video test generator). The load work holds: mean block 715 and 734 µs
(was 1708), `dsp_load_ratio` 0.358 and 0.367 (was 0.854; expected 0.34), every
block ≤ 3 ms, 0 meter drops; the DSP thread 36 % of its CPU, the meter thread
16 % of another; no libm or kernel time left on the DSP thread. Registry and
AMWA (registry on 8110) pass. The load still fails its 0-misses rule: 7090 and
12 445 deadline misses in 60 min, 99 % of them in the seconds the DSP CPU's
sibling was busy (its blocks 3–4 times slower; the CPU clock steady at 3.3 GHz).
The block clock (no DSP work) passed once and failed once (9 misses, 3.9 ms),
as the host's load changed over the day. Their points: the meter thread
allocated per record; `ci/amwa.sh` had the node's port fixed at 3312, taken on
their host.

| # | Decision | Reason |
| --- | --- | --- |
| D124 | Before any other thread starts, the mixer narrows its CPUs for every thread but the DSP thread to those off the DSP CPU's physical core (the CPU and its SMT siblings from sysfs), when it has CPUs on another core; else off the DSP CPU. The DSP thread's notes (`dsp.scheduling`) say "a core of its own (no SMT)", "a core of its own (its other CPUs kept free)", "its core shared with the mixer's other threads (CPU n): no other core" or "its core shared with CPU n, outside this process: another workload may slow it" (also a warning). The worker count is taken from the CPUs before. | The re-run: a busy sibling made nearly all the misses. With whole cores (the static CPU manager's `full-pcpus-only`) the pod gets the sibling, and the mixer's own threads (the meter thread at 16 % of a core, the API, the preview, from 1.1.0 Silero) would have been the ones on it. The note makes a node's layout visible in the status and the load test. |
| D125 | The platform requirement: the DSP CPU's whole physical core (the static CPU manager with `full-pcpus-only`, or SMT off), in the README, the release notes, the deployment example and the runner's README; the load test prints the DSP thread's notes. | A shared core is a property of the node, which the mixer can see but not fix. |
| D126 | The meter thread reads a record's header and channels into fixed arrays (no allocation per record); `ci/amwa.sh` takes the node's ports from `AMWA_NODE_PORT` (3312) and `AMWA_WEB_PORT` (8160) and checks they are free. | Their profile (about 3 % in `malloc`); their host had another node on 3312. |

Checked (dev VM, no SMT): with `taskset -c 1,2,3` the DSP thread runs on CPU 3,
the main thread, both runtime workers and the meter thread on 1–2, and the
status says "CPU 3, a core of its own (no SMT)"; the cases with siblings (a
whole core among others, a sibling outside the process, one core only, no
sysfs) in a unit test. §16 load 60 s: mean block 310 µs, 0.155, as before.
`ci/amwa.sh` with 3312 taken refuses with a message; with `AMWA_NODE_PORT=3412
AMWA_WEB_PORT=8260 AMWA_REGISTRY_PORT=8110`: the same counts, 0 failures, 0
warnings.

### Release 1.0.0: questions after the re-run

| # | Question | Proposal |
| --- | --- | --- |
| 1 | The §16 rule "0 deadline misses" failed only on a shared core. Tag `v1.0.0` with the ratio passing (0.358) and the misses attributed to the shared core in the release notes (as written there), or wait for a run on a node with whole cores? | Merge this PR, then one more `load` and `block-clock` run on its merge commit (this PR changes how the threads are placed), on a whole core if the lab has one; tag on that commit when the ratio passes and the misses are explained. |
| 2 | If no whole-core node is available: tag anyway, with the platform requirement in the notes and the verdict on the misses left to the first production node? | Yes; the work fits its budget with room (0.36 of 0.5), and every sample was correct. |

### Release 1.0.0: answers after the re-run (maintainer, 2026-10-13)

| # | Question | Answer |
| --- | --- | --- |
| 1 | Tag now or wait for a run on whole cores? | Merge PR 20 (merged as `81fb8a2`); the measurements take time: tag and release now to move forward. |

The release (`v1.0.0`) contains PR 20; the lab's run on `81fb8a2` goes into
the release notes when it is in. Then M7a (PR 17) goes into `main`, then M7b.

**Released** 2026-10-13: tag `v1.0.0` on `14e9487`, image
`ghcr.io/leeo86/mxl-audio-mixer:1.0.0` (`1.0`, `1`), digest
`sha256:74753add488e4002a236555f15a943cd75d98bc50d3211b6c1f87241c3bf3f6f`
(labels checked: version 1.0.0, revision `14e9487`, MIT, uid 1000), the GitHub
release with `docs/release-notes/1.0.0.md`. The lab's run on that tag is below.

### Release 1.0.0: the scheduling run (2026-10-13/14)

On `14e9487` (tag `v1.0.0`, checkout `14e9487d81e4`, `target/release` rebuilt,
binary sha256 `e7be6cc7addbbd4f…`), the same host (`NODE_NAME=lab-xeon-6136`,
2× Xeon Gold 6136, kernel 6.8 with voluntary preemption, no isolated CPUs, SMT
on, other media workloads on any CPU). The runner had 9 CPUs of socket 2. The
DSP thread was pinned to the last of them at SCHED_FIFO 49. That CPU's SMT
sibling was not in the set, so a whole core was not available. AMWA ran in its
own network namespace: `AMWA_NODE_PORT=3412`, `AMWA_WEB_PORT=8260`,
`AMWA_REGISTRY_PORT=8110`.

| Item | Result |
| --- | --- |
| env | PASS: TAI offset 37 s, commit `14e9487`, 9 CPUs |
| block clock, 10 min | FAIL by one wake: 1 deadline miss, max wake jitter 2098 µs (limit 2000 µs); p99 31 µs, p99.9 114 µs, p99.99 767 µs |
| §16 load, 60 min | ratio PASS, misses FAIL: 1 798 591 blocks, mean 683 µs, `dsp_load_ratio` 0.342, every block ≤ 5 ms, 1416 deadline misses, 51 168 missing input samples, 0 meter drops, `nr_throttled` +0 |
| AMWA | PASS: IS-04-01, IS-05-01, IS-05-02, BCP-007-03-01, 0 failures |

The load row ends `DSP thread: FTZ/DAZ, timer slack 1 ns, CPU 45, its core
shared with CPU 21, outside this process: another workload may slow it,
SCHED_FIFO 49`. That is the note D124 describes for this host.

Per-second samples of `dsp_deadline_misses_total` against the sibling's busy
share, 3719 s:

| Sibling busy | Seconds | Misses | Per second |
| --- | --- | --- | --- |
| 0–3 % | 1996 | 10 | 0.01 |
| 3–6 % | 277 | 40 | 0.14 |
| 6–10 % | 240 | 89 | 0.37 |
| 10–20 % | 1000 | 1116 | 1.12 |
| ≥ 20 % | 206 | 161 | 0.78 |

1406 of 1416 misses (99 %) fell in seconds another workload used the sibling.
With the sibling idle, 10 misses in 1996 s. The DSP CPU averaged 40 % busy.
198 s below 2.5 GHz carried 6 misses. Against `208689e` (7090 and 12 445
misses) this is 5–9× fewer. The block-clock failure is one wake, 98 µs over
the limit; earlier runs on this host passed twice and failed once. The meter
thread's CPU was not measured (`pidstat` was missing); `meter_drops_total`
stayed 0.

Missing input samples were 960 and 1824 on `208689e` and 51 168 here (533
failed reads: 51 168 / 96). The input read is on the DSP thread
(`src/engine/dsp.rs`). `reserve_dsp_core` narrows threads started after it; it
does not move that read, and the tone writers are other processes. The counter
did not change: after an input has started, every failed `get_samples` adds
the block (96). `input_late_samples_total` is the subset too early or timed
out, and the load line did not print it, so this run cannot say which cause
the 51 168 were. Fewer deadline misses with more missing samples does not fit
"the DSP thread fell behind and the ring overwrote the samples". An automatic
delay change can move the read index (a shorter delay reads closer to the
head and can be too early until the writer catches up); this run did not log
those changes, so that stays a possibility, not a finding.

| # | Decision | Reason |
| --- | --- | --- |
| D131 | `input_overrun_samples_total` counts samples the ring had already overwritten (`OutOfRangeTooLate`). `input_late_samples_total` stays the source that had not written the index (`OutOfRangeTooEarly`, or the quarter-block wait timing out). Other errors stay in `input_missing_samples_total` only. The §16 line prints missing, late source, ring overrun and the remainder. The inputs table shows overrun next to late. | The lab asked for the split. MXL has those two range errors. A wake that still finds the samples is a successful read, so there is no third counter for "reader late" apart from the overrun. |

### Release 1.0.0: question after the scheduling run

| # | Question | Proposal |
| --- | --- | --- |
| 1 | The lab's verdict was to hold the 1.0.0 tag for a whole-core run and for an answer on the missing samples. The tag is already on `14e9487`. | Leave it. The ratio passes (0.342) and the misses match the shared sibling, which this host cannot give up. A whole-core run remains the check of the 0-misses rule. D131 is what the next load prints; it does not move the tag. |

### Release 1.0.0: the split run (2026-10-14)

On `7ea4077` (PR 24), the same host (`NODE_NAME=lab-xeon-6136`, DSP thread on
CPU 45 at SCHED_FIFO 49, sibling CPU 21 outside the runner). `env` passed
(TAI +37 s). The 60 min load: mean block 712 µs, `dsp_load_ratio` 0.356, every
block ≤ 3 ms, 1963 deadline misses, 0 meter drops, not throttled. Missing
input samples **15 840, all late source** (ring overrun 0, other 0): 165 failed
reads. The tone writers of the load test share the runner's CPUs at normal
priority. The reader on the DSP thread did not fall out of the ring, which is
what the earlier 51 168 could not show.

The misses again track the sibling: 12 in 1869 s while it was idle (0–3 %).
The 10–20 % band had 1690 misses in 473 s; the ≥ 20 % band had 8 in 679 s (a
steady neighbour that did not disturb the thread).

Building that commit behind a TLS-inspecting proxy failed in `ort-sys`:
`download-binaries` with `tls-rustls` uses its own root store and rejected the
proxy certificate. The archive they fetched by hand is a raw LZMA2 stream
(`xz --format=raw --lzma2=dict=64MiB`), not a `.tar.xz`.

| # | Decision | Reason |
| --- | --- | --- |
| D132 | The ONNX Runtime download uses the system certificate store (`tls-native` instead of `tls-rustls`). `ORT_LIB_LOCATION` still skips the download: it is the directory that contains `libonnxruntime.a` after the raw LZMA2 archive is extracted. The README says both. | Their build of this commit. A proxy CA in the system store is then trusted. The two TLS features cannot both be on. |

### Release 1.0.0: question after the split run

| # | Question | Proposal |
| --- | --- | --- |
| 1 | The §16 tone writers run at normal priority in the runner's CPU set, and every missing sample on this run was a late source. Give those writers a real-time priority, or pin them off the DSP core, so the load numbers are not the writers'? | Yes, in the load script, once a whole-core node is the one being measured. On this host the writers and the other tenants share the same CPUs either way. |

### Ports (maintainers, 2026-10-14)

`WEB_PORT` 8160 and `NMOS_PORT` 3312 are another function's ports on the same
nodes. There is no Compose file in this repo (the spec's Compose demo is still
listed under deployment, not written); the Kubernetes example and the image
`EXPOSE` are the examples that exist.

| # | Decision | Reason |
| --- | --- | --- |
| D133 | Defaults become `WEB_PORT` 8190 and `NMOS_PORT` 3342 (events WebSocket 3343). Called out as a breaking default in the 1.1.0 notes, the changelog, the README and the specification. The §16 load keeps 8180/3332. | Both are free on the target nodes, and they do not meet the load test's ports. 1.0.0's numbers stay in that release's notes. |

## M8a: logic graphs

Milestone 8 (§19, release 1.2.0) in parts. This part: graphs of blocks in the
show, evaluated at 100 Hz, with the blocks a show needs first and a list
editor. The cable canvas and the remaining block types follow.

### M8a: decisions

| # | Decision | Reason |
| --- | --- | --- |
| D127 | A logic graph is blocks and wires in the show (`logic`), at most 32 graphs of 64 blocks. A value is a number; a boolean is 0 or 1. Ports are fixed per block type; a wire goes from an output to an input and an input takes one wire. A loop is rejected unless it passes through an `edge` or a `latch`: those publish last tick's value, so what follows them is one tick behind. | §11.1 "typed ports"; §11.2 "combinational loops are rejected unless they contain a latch/delay". |
| D128 | The blocks: `const`, `button` (a soft key, held in the runtime, not the show), `mute`, `fader`, `vad`, `level` (a signal's mute, fader dB, voice and peak dBFS), `and`, `or`, `not`, `compare` (`gt`, `lt`, `eq`), `edge` (1 for the tick its input rose), `latch` (set/reset, reset wins), `set-mute`, `set-fader`. A `vad` block analyses its signal like an auto-level does. | §11.1's list, the part that drives a show: "while the host speaks, mute the music bed", "below −40 dBFS, close the fader". Tally, timers, schedules, NMOS state, webhooks, snapshot recall and math follow (question 2). |
| D129 | Evaluation is on the control engine's 10 ms tick (100 Hz, §11.2's default). Actions run when their input changed, ramped over 10 ms, and never while `dry_run` is set. A fader they ask for is clamped to the fader range. Deleting a signal removes the blocks that name it. | The tick already exists. Acting on every tick would fight the operator; acting on change does what the graph says once. |
| D130 | The Logic tab is a list: graphs, blocks, wires by their ports, live values, the soft keys. Positions (`x`, `y`) are stored for the canvas. | The canvas (drag, cables, zoom) is the larger piece and comes next (question 1); the runtime is what a show runs on. |

### M8a: questions

| # | Question | Proposal |
| --- | --- | --- |
| 1 | The cable canvas (Vue Flow, as the routing graph) as the next part, the list editor staying for now. | Yes. |
| 2 | Next blocks: tally on air, a timer, compare against a meter with hysteresis, snapshot recall. Webhooks and schedules after those. | In that order. |
| 3 | Logic stays out of snapshots (it is the show, not a parameter state). | Yes. |
| 4 | 100 Hz on the existing tick, not a thread of its own. | As implemented. |

### M8a: answers (maintainer, 2026-10-13)

| # | Question | Answer |
| --- | --- | --- |
| 1 | The cable canvas next, the list editor staying | Yes. |
| 2 | Next blocks: tally, a timer, meter hysteresis, snapshot recall | In that order. |
| 3 | Logic stays out of snapshots | Yes. |
| 4 | 100 Hz on the existing tick | As implemented. |

The canvas is on the Logic tab (Vue Flow, the blocks' `x` and `y`): draw a cable from an output to an input, Delete removes it, dragging a block remembers where it was put. The list under it edits the same graph.

### M8a: tally on air

A `tally` block takes a TSL tally index (the same number a strip's `tally_index` uses, after `TSL_MAP`). Its output is 1 while that index's left lamp is lit, and 0 before any tally and while the lamp is off. The last lamp stays when packets stop, as audio-follows-video does (D58).

| # | Decision | Reason |
| --- | --- | --- |
| D134 | The block is on air only (the left lamp), not preview. The index is 0–65535, stored on the block. | Answer 2: tally on air is the next block. Preview is the right lamp; AFV does not use it either (§10.3). |

### M8a: a timer

A `timer` block is the pulse in §11.1. `out` is 1 for `ms` milliseconds after `in` rises, then 0. The length is rounded up to the 10 ms tick, so it is at least the duration asked and at most one tick longer. A rise while it is still on starts the duration again. It remembers, so a loop may pass through it. Zero stays off. The longest duration is one day.

| # | Decision | Reason |
| --- | --- | --- |
| D135 | The timer is a pulse on the rising edge, not an off-delay and not a clock. `ms` is 0 to 86 400 000, rounded up to ticks of the existing 10 ms evaluation. | Answer 2: a timer is the block after tally. §11.1 lists timer/pulse under logic, and schedules after these blocks. The tick is already 10 ms (D129). |

### M8a: meter hysteresis and snapshot recall

An `above` block reads a signal's peak (dBFS). Its output becomes 1 when the peak reaches `threshold_db` (−120 to 20) and stays 1 until the peak falls below that by `hysteresis_db` (0 to 60). Silence, and a signal with no meter yet, read as −200 dBFS, so the block stays off.

A `recall` block names a snapshot. It runs that snapshot when its input rises. The first tick records the input and does not recall, so a graph that loads with the input already on does not recall by itself. The ramp is the same 10 ms as the other logic actions. Recall-safe signals stay as they are. A missing snapshot is left alone and logged. If the recall rewires the plan, the nodes fade out and the plan swaps on a later tick.

| # | Decision | Reason |
| --- | --- | --- |
| D136 | Hysteresis is a threshold and a gap below it, on the signal's peak, in its own block. It turns on at the threshold and off `hysteresis_db` lower. | Answer 2: compare against a meter with hysteresis. A plain compare chatters on a noisy meter. The raw `level` block stays, for a number. |
| D137 | Recall fires on a rising edge only, after the first tick has been seen. It recalls the whole snapshot (recall-safe signals excepted) over the 10 ms logic ramp. | Answer 2: snapshot recall is the action. Recalling because a constant was already 1 when the mixer started would change a show unasked. |

### Milestone 9: remote link (first slice)

One master, up to four slaves, on the mixer's own web port (`/api/v1/link`), token in the first message (not shown by `GET /api/v1/config`). The slave sends its revision and the faders and mutes it is on. The master queues mute, fader, recall and a push of those levels. Audio stays on MXL. When the master drops, the slave keeps the mix; with takeover `deny`, local changes are refused until the master is back (after it had connected once, so start-up still loads the show).

| # | Decision | Reason |
| --- | --- | --- |
| D138 | The link is a WebSocket on `WEB_PORT`, not a second listener. Up to 4 peers. Takeover deny blocks `stage` only after a master has connected and then gone. | §13.3 "one master, up to 4 slaves", "holds its last state", "local takeover allowed/denied". TLS is the platform's in front of the web port. |
