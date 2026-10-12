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
