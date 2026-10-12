# mxl-audio-mixer

A broadcast audio engine on MXL, in Rust: a freely routable audio matrix, channel
strips, broadcast DSP, loudness, a control room, VCAs and master/slave operation.
The contract is [`SPECIFICATION.md`](SPECIFICATION.md); decisions, deviations and
results are in [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md).

**Status: milestone 4** (§19). Settings, probes, metrics, the NMOS node with its
MXL audio receivers, and the inputs read on the TAI block clock (waiting, stalled
and re-created flows, arrival lag and automatic delay); the mix: input patch,
strips (trim, polarity, fader, mute, pan/balance/width), buses, a send matrix with
width mapping, and buses published as MXL flows with NMOS senders; processing
chains (EQ, filters, dynamic EQ, compressor, limiter, expander, gate, de-esser,
ducker, multiband, delay) with delay compensation and block-delay feedback;
peak, peak hold, true peak, RMS, PPM, correlation, gain-reduction and loudness
(BS.1770-4 / R 128) meters and an analyser; snapshots, layouts and the show
history; the web UI (console, processing editor, routing matrix, snapshots,
metering) with live updates over a WebSocket.

## Web UI

`http://<host>:8160/`: one page built into the binary (no CDN, no other files). Tabs
(the URL hash addresses them): **Console** (strips in banks or in a layout's pages:
patch, trim, polarity, the chain overview with its response and gain reduction,
sends, pan and width, mute, fader, meter; the buses beside them), **Processing**
(the chain of a strip or bus: drag node types in from the palette, drag nodes to
reorder, bypass, delete; EQ curve with band handles, filter, dynamics transfer
curves with the live level, de-esser listen, keys), **Routing** (the send matrix,
a send's tap, gain, mute, phase and block delay; publishing outputs),
**Snapshots** (take, recall with filters, recall-safe, crossfade; layouts; show
history), **Meters** (the bridge on dBFS, true peak or PPM EBU, BBC, Nordic, DIN at
an alignment level, phase correlation; loudness; spectrum and goniometer of any
tap), **NMOS**, **Status**, **Settings** (mix settings, configuration, show export
and import). Every client follows every change. Faders and pots take touch: drag,
− and + (held: repeated), double-click resets; destructive actions want a second
press within 3 s. The theme follows the system or is set light or dark (kept in
the browser).

`scripts/demo.sh` starts a tone flow, the mixer and a small show to try it.

## Run

```sh
scripts/build-mxl.sh                               # libmxl at the pin, into /opt/mxl
(cd web && npm ci && npm run build)                # the web UI (Node 22); without it the binary serves a note
cargo build --release
LD_LIBRARY_PATH=/opt/mxl/lib NMOS_HOST_ADDRESS=192.0.2.10 MXL_DOMAIN_SCAN_PATH=/dev/shm/mxl \
    MIXER_STATE_DIR=/tmp/mixer target/release/mxl-audio-mixer
```

The image is `ghcr.io/leeo86/mxl-audio-mixer` (uid 1000; tags `X.Y.Z`, `X.Y`, `X`,
`git-<sha7>`, `nightly-dev`). `deploy/k8s/deployment.yaml` shows the platform
conventions; `deploy/grafana/` has a dashboard. The development set-up is in
[`docs/development.md`](docs/development.md).

On the platform: the MXL root at `/Volumes/mxl` (hostPath), a writable `/config`,
the pod IP in `NMOS_HOST_ADDRESS`, the registry in `NMOS_REGISTRY_ADDRESS` /
`NMOS_REGISTRY_PORT`, and the node's kernel TAI offset set (CLOCK_TAI = UTC + 37 s).
Guaranteed QoS with whole CPUs for the DSP thread; `MIXER_RT_PRIORITY` (≤ 49) needs
the `SYS_NICE` capability.

## Settings

Environment first, then the JSON file `MIXER_CONFIG_FILE` (an object of these keys),
then the defaults. Unknown environment variables are ignored; unknown file keys and
invalid values exit with code 78. `GET /api/v1/config` lists every value with its
origin. No setting is a secret.

| Setting | Default | Meaning |
| --- | --- | --- |
| `HOST_ID` | the host name | Instance name; the default node label and the start of the default NMOS seed. |
| `WEB_PORT` | 8160 | Port of the web UI, the API, /livez, /readyz and /metrics. |
| `NMOS_PORT` | 3312 | Port of the IS-04 Node API and the IS-05 Connection API. |
| `NMOS_SEED` | <HOST_ID>-mixer | Seed of every NMOS id, the output domain id and the MXL flow ids (UUIDv5). |
| `NMOS_LABEL` | <HOST_ID> | Node label; the Device is "<label> Audio Mixer". |
| `NMOS_TAGS` | {} | JSON object of tag name to string arrays, on the Node and the Device. |
| `NMOS_HOST_ADDRESS` | first non-loopback IPv4 | IPv4 literal announced in NMOS (the pod IP on the platform). |
| `NMOS_REGISTRY_ADDRESS` | unset | IPv4 address or DNS name of the IS-04 Registration API. Unset: no registration. |
| `NMOS_REGISTRY_PORT` | unset | Port of the Registration API (set together with the address). |
| `NMOS_QUERY_ADDRESS` | registry address | IPv4 address or DNS name of the Query API, used for readiness. |
| `NMOS_QUERY_PORT` | registration port + 1 | Query API port. |
| `NMOS_DNS_SD` | false | mDNS/DNS-SD browsing and advertisement (needs a build with the dns-sd feature). |
| `MXL_DOMAIN_SCAN_PATH` | /Volumes/mxl | Parent of the MXL domain directories; input domains (mirrors included) are found below it. |
| `MXL_OUTPUT_DOMAIN_DIR` | <scan path>/mixer-<first 8 hex of the domain id> | The mixer's own output domain. Created if missing. |
| `MXL_OUTPUT_DOMAIN_ID` | UUIDv5 of NMOS_SEED | Id of the own output domain. |
| `MXL_HISTORY_DURATION_NS` | 200000000 | history_duration written into a new output domain. |
| `MXL_CLEANUP_ON_EXIT` | false | Remove the own output domain directory on SIGTERM. |
| `SHUTDOWN_TIMEOUT_S` | 10 | Time for the SIGTERM sequence (1–120). |
| `MIXER_CONFIG_FILE` | unset | JSON file with any of these settings (environment wins). |
| `MIXER_STATE_DIR` | /config | Directory of everything the mixer writes itself (show file, connections). |
| `MIXER_LOG_LEVEL` | info | Log level: error, warn, info, debug, trace. |
| `MIXER_INPUT_FLOWS` | 16 | Number of MXL audio inputs (NMOS receivers), 1–256. |
| `MIXER_BLOCK_SAMPLES` | 96 | DSP block size in samples at 48 kHz: 48, 96, 144 or 240. |
| `MIXER_READ_OFFSET` | 2 | Read offset in ms: every input is read at t_out − (offset + its extra delay). |
| `MIXER_OUTPUT_COMMIT_SAMPLES` | one block | Samples per MXL commit of an output (a multiple of the block size). |
| `MIXER_RT_PRIORITY` | off | SCHED_FIFO priority of the DSP thread, 1–49 (needs CAP_SYS_NICE). |
| `MIXER_INPUT_STALL_S` | 2 | An input whose head index does not move for this long is stalled and re-opened. |
| `MIXER_INPUT_DELAY_MAX_MS` | 150 | Upper limit of an input's automatic extra delay. |
| `MIXER_INPUT_DELAY_CONFIRM` | false | true: an automatic delay change waits for operator confirmation instead of applying with a fade. |

Per input (show file, `PATCH /api/v1/inputs/<n>`): `label` (NMOS receiver label and
group hint, default `Input <n>`) and `extra_delay` (`"auto"`, the default, or ms).
`auto` measures the arrival lag (TAI now minus the flow's head index, sliding window)
over the last 10 s and sets the extra delay to its 99.9th percentile plus 1 ms,
minus the read offset, within `MIXER_INPUT_DELAY_MAX_MS`; a change is applied with a fade, or waits
for `POST /api/v1/inputs/<n>/delay/confirm` with `MIXER_INPUT_DELAY_CONFIRM=true`.

## The mix

Kept in `MIXER_STATE_DIR/mix.json` and in the show file. Signal ids (strips and
buses share one namespace) are 1–32 characters `a-z`, `0-9`, `-`; a new one without
an id gets `strip-<n>` or `bus-<n>`.

- **Strip:** patch → trim and polarity → `chain` → fader and mute → panner.
  `patch` has one entry per strip channel: `{"input": n, "channel": c}` (both from
  1) or `null`. A channel the input does not have is silent and shows `missing` in
  `patch_state`. `trim_db` ±42, `fader_db` `"-inf"` … +10, `pan` −1 … +1 (mono:
  position; stereo: balance), `width` 0 … 2 (stereo; 1 unchanged), `loudness`
  (measure it). A mono strip is stereo after its panner.
- **Bus:** sum of its sends → `chain` → fader and mute; `loudness`;
  `compensate` (delay compensation, default `true`).
- **Taps** (sends and keys): `input` (strips), `pre-eq`, `post-eq`,
  `pre-dynamics`, `post-dynamics` (the input of the first and the output of the last
  node of that kind; the chain's input without one), `pre-fader` (the chain's
  output), `post-fader`, `post-pan` (strips), or `{"after": "<node id>"}`.
- **Send** (`from` a strip or bus `to` a bus): `tap`, `gain_db` (`"-inf"` … +10),
  `mute`, `phase`, `block_delay`. New sends default to 0 dB from a strip's post-pan
  or a bus's post-fader. A loop is rejected unless one of its sends has
  `block_delay: true` (it reads its source one block late, §4.4).
- **Delay compensation** (§4.4): the sends into a bus are delayed to the largest
  latency among them (look-ahead limiters), so parallel paths stay phase-coherent.
  Keys and block-delay sends are not compensated. Each output reports its
  `processing_latency_ms` and `latency_ms` (read offset + one block + processing).
- **Output:** a bus published as an `audio/float32` MXL flow in the own domain and
  an NMOS Source, Flow and Sender (label = bus name, group hint `<name>:Audio`).
  `format` (default: the bus's) maps it to another layout; `commit_samples`
  (default `MIXER_OUTPUT_COMMIT_SAMPLES`) is a multiple of the block size. The MXL
  flow id is UUIDv5 of `NMOS_SEED` and the bus id, so it survives restarts. At most
  128 outputs.
- **Formats** (§4.1), channel order as SMPTE ST 2110-30: mono `M`; stereo `L R`;
  `3.0` `L R C`; `5.1` `L R C LFE Ls Rs`; `7.1` `L R C LFE Lss Rss Lrs Rrs`;
  `5.1.4` = 5.1 + `Ltf Rtf Ltr Rtr`; `7.1.4` = 7.1 + the same heights.
- **Width mapping** (sends and outputs): same channels map 1:1; missing ones fold
  down at −3 dB per step (ITU-R BS.775; LFE dropped): C → L + R; Ls → L (or Lss);
  Lss, Lrs → Ls; Ltf → L; Ltr → Lrs (or Ls); stereo → mono with the stereo-to-mono
  sum; mono → C (3.0 and the surround formats) or L + R with the pan law
  (stereo); channels the source lacks stay silent.
- **Mix settings:** `pan_law_db` 0, −3 (default), −4.5, −6; `mono_sum_db` −3
  (default) or −6; `loudness_target_lufs` (−23) and `loudness_tolerance_lu` (1).
- **Console:** `colour` (`#rrggbb`) and `recall_safe` on strips and buses;
  `layouts`: `[{"name", "pages": [{"name", "signals": [ids]}]}]`.
- **Snapshots** (§12.5): parameter states of every strip, bus and send (trim,
  polarity, fader, mute, pan, width, every chain node's parameters and bypass,
  send gain, mute and phase). A recall takes `signals`, `sections` (`strips`,
  `buses`) and `classes` (`trim`, `fader`, `mute`, `pan`, `chain`, `sends`), all
  optional (everything), and `crossfade_ms` (the ramp of every value); it never
  touches recall-safe signals, nor signals, sends and nodes that no longer exist
  (or whose node type changed). Snapshots and layouts are part of the show.
- **Show history:** `MIXER_STATE_DIR/history/`: an automatic version at most once a
  minute after a change (the newest 100 kept), named versions on request (kept
  until deleted). A restore or an import keeps the current show as a version first.

Parameter changes ramp over `ramp_ms` (default 10 ms). A change that rewires the
graph fades the affected paths out and in (10 ms) around the plan swap; unrelated
paths are not touched, and processing nodes keep their state. Parameters are
addressed by signal id and field (§12.4), e.g. `strips/vox/fader_db`,
`sends/vox/main/gain_db`, `strips/vox/chain/eq-1/bands/0/gain_db`.

### Processing nodes (§6)

Every node has an `id` (unique in its chain; `<type>-<n>` when omitted), a `type`,
`bypass` (crossfaded over one block; the dry path is delayed by the node's
latency) and these parameters, all required. Numeric parameters ramp; the others
change the node's structure. Dynamics detect on the largest level of all channels
(linked); attack and release are time constants (63 % of a step).

| `type` | Parameters |
| --- | --- |
| `eq` | `bands`: 1–8 of `{"type": "bell" \| "low-shelf" \| "high-shelf" \| "notch", "freq_hz", "q", "gain_db"}` (RBJ cookbook; gain ±24 dB, Q 0.1–40) |
| `filter` | `mode` `high-pass` \| `low-pass`, `freq_hz`, `slope_db_per_oct` 12 / 24 / 36 / 48 (Butterworth) |
| `dynamic-eq` | `bands`: 1–8 of `{freq_hz, q, gain_db, threshold_db, ratio, attack_ms, release_ms}`: a bell whose gain drops by a compressor on the band's level |
| `compressor` | `threshold_db`, `ratio` (1–1000), `knee_db` (soft knee, 0–24), `attack_ms`, `release_ms`, `makeup_db`, `detector` `peak` \| `rms`, `sidechain_hpf_hz` (optional), `key` `{signal, tap}` (optional) |
| `limiter` | `ceiling_db`, `release_ms`, `lookahead_ms` (0.1–20, the node's latency), `true_peak` (detect on 4× oversampled peaks) |
| `expander` | `threshold_db`, `ratio`, `range_db`, `attack_ms`, `release_ms` |
| `gate` | `threshold_db`, `hysteresis_db` (closes that far below), `attack_ms`, `hold_ms`, `release_ms`, `range_db` |
| `de-esser` | `mode` `split-band` (reduces the band above `freq_hz`) \| `wide-band` (reduces everything), `freq_hz` 2000–12000, `threshold_db`, `range_db`, `attack_ms`, `release_ms`, `listen` (output the band) |
| `ducker` | `key` `{signal, tap}`, `threshold_db`, `depth_db`, `attack_ms`, `hold_ms`, `release_ms` |
| `multiband` | `crossovers_hz` (2–4, rising; Linkwitz-Riley 24 dB/oct, sum flat), `bands` (one more: `{threshold_db, ratio, attack_ms, release_ms, makeup_db}`) |
| `delay` | `time`, `unit` `samples` \| `ms` \| `frames`, `frame_rate` (frames); at most 2000 ms |

Ranges: thresholds −100 … 0 dB, attack 0–1000 ms, release 0–10 000 ms, makeup
±24 dB, a chain at most 32 nodes. A key is another signal's tap; its signal is
processed first.

### Loudness and meters (§7)

`loudness: true` on a strip or bus measures it post-fader (ITU-R BS.1770-4): momentary
(400 ms), short-term (3 s), both updated every 10 ms, integrated (gated, from the
start of the measurement) and loudness range (EBU Tech 3342), with the largest true
peak. The measurement runs from when it is switched on; it can be stopped, reset and
started again. It continues over changes of the mix; blocks the DSP thread missed
count as silence.

`GET /api/v1/meters` gives, per strip and bus (post-fader) and channel: sample peak
(since the last 40 ms update), peak hold (until cleared), true peak (4×
oversampling), RMS (300 ms) and the PPM readings (IEC 60268-10 Type I: 5 ms
integration, 20 dB in 1.7 s; Type II: 10 ms, 24 dB in 2.8 s; a steady 1 kHz tone
reads its peak); the phase correlation of the first two channels (300 ms); per
processing node: gain reduction and the peaks at its input and output. The UI draws
the PPM scales (EBU, BBC, Nordic, DIN) at an alignment level (−18 dBFS, EBU R 68,
or −20 dBFS).

The **analyser** reads one tap point at a time (`PUT /api/v1/analysis`): a
4096-point spectrum (Hann) in 1/6-octave bands from 20 Hz to 20 kHz (the mean power
of the first two channels; a full-scale sine reads 0 dB), their correlation and
256 sample pairs for the goniometer, every 40 ms. Changing it swaps the plan
without touching the audio.

## Ports and exit codes

| Port | Default | |
| --- | --- | --- |
| `WEB_PORT` | 8160 | web UI, API, events WebSocket, probes, metrics |
| `NMOS_PORT` | 3312 | IS-04 Node API, IS-05 Connection API |

| Exit code | Meaning |
| --- | --- |
| 0 | stopped by Ctrl-C |
| 75 | a port cannot be bound, or the DSP thread cannot start |
| 78 | invalid configuration, libmxl missing, output domain id clash |
| 143 | SIGTERM: media stopped, MXL released, node deregistered, own domain removed with `MXL_CLEANUP_ON_EXIT=true`, within `SHUTDOWN_TIMEOUT_S` |

## API

| Method and path | |
| --- | --- |
| `GET /` | the web UI |
| `GET /api/v1/events` (WebSocket) | live updates, below |
| `GET /livez` | 200 while the process runs; never depends on the registry |
| `GET /readyz` | 200 when the engine runs and, with a registry, the node is registered and listed by the Query API; JSON details incl. `timing_trusted` |
| `GET /metrics` | Prometheus text, prefix `mxl_audio_mixer_` |
| `GET /api/v1/status` | version, readiness, NMOS, MXL domain, DSP, every input, the mix's size, every output |
| `GET /api/v1/config` | every setting: value, origin (`environment`, `file`, `default`), default, description |
| `GET /api/v1/config/export` | the show file: settings (for the record), inputs, mix, active connections |
| `POST /api/v1/config/import` | restores the inputs, the mix and the connections (through IS-05); settings are not applied |
| `GET /api/v1/mix` | the whole mix |
| `GET`, `PATCH /api/v1/mix/settings` | `{"pan_law_db": …, "mono_sum_db": …}` |
| `GET`, `POST /api/v1/strips` | list (with `patch_state`), create (`format` required) |
| `GET`, `PATCH`, `DELETE /api/v1/strips/<id>` | any strip field, `ramp_ms`; delete removes its sends |
| `GET`, `POST /api/v1/buses` | list, create (`format` required) |
| `GET`, `PATCH`, `DELETE /api/v1/buses/<id>` | `name`, `format`, `fader_db`, `mute`, `chain`, `loudness`, `compensate`, `colour` (`""` clears it), `recall_safe`, `ramp_ms`; delete removes its sends and output |
| `GET /api/v1/sends` | every send |
| `GET`, `PUT`, `DELETE /api/v1/sends/<from>/<to>` | create or change: `tap`, `gain_db`, `mute`, `phase`, `ramp_ms` |
| `GET /api/v1/outputs`, `GET /api/v1/outputs/<bus>` | published outputs: format, channels, commit size, MXL flow id, NMOS sender id, blocks, commits, errors, commit lag |
| `PUT`, `DELETE /api/v1/outputs/<bus>` | publish (`{"format": …, "commit_samples": …}`, both optional) or unpublish |
| `GET`, `POST /api/v1/{strips,buses}/<id>/chain` | the chain; add a node (its JSON, `id` optional, `position` optional) |
| `GET`, `PATCH`, `DELETE /api/v1/{strips,buses}/<id>/chain/<node>` | a node; change it with a JSON merge patch (RFC 7396) plus `ramp_ms` |
| `POST /api/v1/{strips,buses}/<id>/chain/<node>/move` | `{"position": n}` |
| `GET /api/v1/loudness` | per measured signal: `momentary_lufs`, `short_term_lufs`, `integrated_lufs`, `range_lu`, `max_true_peak_dbtp`, `running`, `in_tolerance` |
| `POST /api/v1/loudness/<id>/{start,stop,reset}` | control the integration |
| `GET /api/v1/meters` | per strip and bus (post-fader), per channel: `peak_dbfs` (since the last 40 ms update), `peak_hold_dbfs` (highest peak until cleared), `true_peak_dbtp`, `rms_dbfs` (300 ms), `ppm_type1_dbfs`, `ppm_type2_dbfs`; `correlation` (two or more channels); `nodes`: per node `gain_reduction_db`, `in_peak_dbfs`, `out_peak_dbfs` |
| `POST /api/v1/meters/clear`, `POST /api/v1/meters/<id>/clear` | clear every peak hold, or one signal's |
| `GET`, `PUT`, `DELETE /api/v1/analysis` | the analyser: `{"signal", "tap"}` (tap default `post-fader`); GET gives `target` and `values` (`bands_hz`, `spectrum_db`, `correlation`, `xy`) |
| `GET`, `POST /api/v1/snapshots` | list; take one (`{"id"?, "name"?}`) |
| `GET`, `PUT`, `PATCH`, `DELETE /api/v1/snapshots/<id>` | one snapshot; PUT takes it again from the current mix; PATCH `{"name"}` |
| `POST /api/v1/snapshots/<id>/recall` | `{"signals"?, "sections"?, "classes"?, "crossfade_ms"?}` |
| `GET`, `PUT /api/v1/layouts` | the console layouts (the whole list) |
| `GET`, `POST /api/v1/shows/history` | versions, newest first (`id`, `time_ms`, `name`, sizes); keep the current show as a named version (`{"name"}`) |
| `GET`, `DELETE /api/v1/shows/history/<id>`, `POST …/<id>/restore` | a version's mix; delete; restore it as the show |
| `GET /api/v1/inputs`, `GET /api/v1/inputs/<n>` | state (`unrouted`, `waiting`, `running`, `stalled`), flow, channels, delays, arrival lag, late/missing samples, re-opens |
| `PATCH /api/v1/inputs/<n>` | `{"label": …, "extra_delay": "auto" \| ms}` |
| `POST /api/v1/inputs/<n>/delay/confirm` | apply a proposed automatic delay |
| `/x-nmos/…` on `NMOS_PORT` | IS-04 v1.3, IS-05 v1.2, BCP-007-03 ([mxl-nmos](https://github.com/LeeO86/mxl-nmos) v0.1.1) |

### Live updates

`/api/v1/events` sends JSON messages: `{"type": "status", "status": …}` five times a
second (as `GET /api/v1/status`); `{"type": "mix", "revision", "mix", "latency_ms"}`
on connect and within 40 ms of every change of the mix, from any client; `{"type": "meters",
"meters", "loudness"}` every 40 ms; and `{"type": "analysis", …}` for clients that
asked. A client may send `{"type": "subscribe", "meters": [ids] | null, "meter_hz":
25, "analysis": false}` to receive only some signals' meters, less often, or the
analyser. Changes go through the REST API.

## Metrics

`dsp_block_seconds` (histogram), `dsp_deadline_misses_total`, `dsp_load_ratio`,
`plan_swaps_total`, `plan_compile_seconds` (last compile), `signals`,
`nodes{type}` (steps of the running plan), `output_blocks_total`,
`output_commit_lag_ms{output}`, `loudness_lufs{signal,window}` (window
`momentary`, `short-term`, `integrated`), `true_peak_dbtp{signal}`,
`timing_trusted`, and per input: `input_state`,
`input_lag_ms`, `input_arrival_lag_ms` (quantiles 0.5, 0.99, 0.999),
`input_effective_delay_ms`, `input_late_samples_total`,
`input_missing_samples_total`, `input_reopens_total`; `process_cpu_seconds_total`,
`process_resident_memory_bytes`. All prefixed `mxl_audio_mixer_`.

## Tests

`cargo test` (unit, and EBU Tech 3341 cases 1–23 and Tech 3342 cases 1–6 on the EBU
test set with `EBU_TEST_SET`, see `docs/development.md`),
`tests/integration/lifecycle.sh` (G14: start, ready, SIGTERM,
exit 143, deregistered, own domain removed; connections survive a restart),
`tests/integration/inputs.sh` (§17: activation before the flow exists, stalled and
re-created writers, a writer 30 ms late), `tests/integration/mix.sh` (the mix
end to end: sample-exact output checks with `mxl-mixer-probe`, plan swaps that
leave other paths untouched, meters, the NMOS sender, a restart with a downstream
reader, unpublish, loop rejection), `tests/integration/processing.sh` (EQ
sample-exact, ramped parameters without a plan swap, bypass, high-pass, compressor
gain reduction, limiter ceiling and latency, loudness and its controls),
`tests/integration/show.sh` (snapshots with filters, recall-safe and crossfade,
layouts, show history, PPM, correlation, the analyser, the events WebSocket),
`tests/integration/ui.sh` (the UI in headless Chrome with two clients: every action
checked on the API and on the other client), `tests/perf/section16.sh` (the
§16-shaped load), `ci/amwa.sh off` (AMWA IS-04-01, IS-05-01,
IS-05-02, BCP-007-03-01 against the mixer with a published bus). Hardware tests for
the maintainers are in `tests/hw/`.

`mxl-mixer-tone` (a test tone writer) and `mxl-mixer-probe` (a reader that checks
a flow against the tone) are in the image too.

## Licence

MIT. Dependencies: `mxl` / `mxl-sys` (Apache-2.0, dmf-mxl), `mxl-nmos`
(MIT OR Apache-2.0), and MIT or Apache-2.0 crates. The UI: Vue (MIT), built with
Vite (MIT); its base styles and components come from the sibling media functions
(MIT, same author).
