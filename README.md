# mxl-audio-mixer

A broadcast audio engine on MXL, in Rust: a freely routable audio matrix, channel
strips, broadcast DSP, loudness, a control room, VCAs and master/slave operation.
The contract is [`SPECIFICATION.md`](SPECIFICATION.md); decisions, deviations and
results are in [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md).

**Status: milestone 5 in progress** (§19). Settings, probes, metrics, the NMOS node with its
MXL audio receivers, and the inputs read on the TAI block clock (waiting, stalled
and re-created flows, arrival lag and automatic delay); the mix: input patch,
strips (trim, polarity, fader, mute, pan/balance/width), buses, a send matrix with
width mapping, and buses published as MXL flows with NMOS senders; processing
chains (EQ, filters, dynamic EQ, compressor, limiter, expander, gate, de-esser,
ducker, multiband, delay) with delay compensation and block-delay feedback;
peak, peak hold, true peak, RMS, PPM, correlation, gain-reduction and loudness
(BS.1770-4 / R 128) meters and an analyser; snapshots, layouts and the show
history; the web UI (console, processing editor, routing matrix, snapshots,
metering) with live updates over a WebSocket; widgets for operator screens
(`strip`, `fader-bank`, `loudness`, `monitor`); tally (TSL UMD 5.0) and
audio-follows-video; the control room (monitors, PFL/AFL, solo-in-place, MXL
monitor outputs, oscillator).

## Web UI

`http://<host>:8160/`: one page built into the binary (no CDN, no other files). Tabs
(the URL hash addresses them): **Console** (strips in banks or in a layout's pages:
patch, trim, polarity, the chain overview with its response and gain reduction,
sends, pan and width, mute, fader, meter; the buses beside them), **Processing**
(the chain of a strip or bus: drag node types in from the palette, drag nodes to
reorder, bypass, delete; EQ curve with band handles, filter, dynamics transfer
curves with the live level, de-esser listen, keys), **Routing** (the send matrix,
a send's tap, gain, mute, phase and block delay; publishing outputs), **Graph**
(the routing as a graph, read-write: inputs, strips, buses, monitors and outputs
with their patches, sends, mix-minus, monitor sources and outputs; draw from a
strip or bus to a bus to send, click a send to edit it, Delete removes it; nodes
in columns by routing depth, moved ones kept in the browser),
**Snapshots** (take, recall with filters, recall-safe, crossfade; layouts; show
history), **Meters** (the bridge on dBFS, true peak or PPM EBU, BBC, Nordic, DIN at
an alignment level, phase correlation; loudness; spectrum and goniometer of any
tap), **NMOS**, **Status**, **Settings** (mix settings, configuration, show export
and import). Every client follows every change. Faders and pots take touch: drag,
− and + (held: repeated), double-click resets; destructive actions want a second
press within 3 s. The theme follows the system or is set light or dark (kept in
the browser).

`scripts/demo.sh` starts a tone flow, the mixer and a small show to try it.

## Widgets

For operator screens built by the production designer (§12.6, Appendix B.2), as
in the sibling media functions. `GET /widgets` lists them with a JSON schema of
their query (CORS only for the origins in `WIDGET_FRAME_ANCESTORS`, `Vary:
Origin`). `GET /widget/<id>?<params>[&theme=dark|light|transparent]` is the page:
the widget without the app, on the mixer's own API, with `Content-Security-Policy:
frame-ancestors <WIDGET_FRAME_ANCESTORS>` and no `X-Frame-Options`. A bad parameter
is 400, an unknown widget 404, another method than GET 405. The page posts
`{type: "widget-ready"}` and `{type: "widget-size", w, h}` to its parent.

| id | Params | Shows and controls | min_size |
| --- | --- | --- | --- |
| `strip` | `strip` (a strip or bus id) | name, tally, gain reduction per dynamics node, PFL, mute, fader, meter | 120×480 |
| `fader-bank` | `layout` (a layout's name) and `page` (its name or 1-based number), or `strips` (strip, bus or VCA ids, comma-separated, 1–32) | the strips, buses and VCAs of that page, side by side | 480×480 |
| `loudness` | `signal` (a strip or bus id) | momentary, short-term, integrated LUFS, LRA, max true peak, the tolerance; start, stop, reset | 320×200 |
| `monitor` | `monitor` (a monitor id) | source buttons, level, dim, cut, mono, the meter | 360×200 |
| `talkback` | `destinations` (ids, comma-separated; default all), `mic` (a talkback mic id; default the browser's microphone) | talk buttons (a short press latches, hold to talk), "All", the mic's meter; with the browser's microphone its on/off | 260×110 |
| `vca` | `vca` (a VCA id) | the VCA's fader, mute, PFL; spill shows its members beside it | 120×480 |

The strip widget has PFL. The talkback
widget's browser microphone needs an HTTPS page and a frame with
`allow="microphone"`.

## Tally and audio-follows-video

With `TSL_ENABLE=true` the mixer receives TSL UMD 5.0 from the platform's tally
calculator on `TSL_UDP_PORT` and `TSL_TCP_PORT` (TCP with DLE/STX framing; UTF-16
labels), as mxl-multiviewer and mxl-webrtc-monitor do. Messages of `TSL_SCREEN`
(−1: every screen) set the lamps of a tally index: `TSL_MAP` maps display indexes
to tally indexes (empty: the same number; with a map, unmapped displays are
ignored). LH lit is on air, RH lit is on preview.

A strip follows a tally index (`tally_index`). With `afv` (`{"enabled", "depth_db",
"fade_in_ms", "fade_out_ms"}`, fades 0–10 000 ms, depth `"-inf"` … 0) it is open
while its index is on air and at `depth_db` otherwise, also before any tally has
arrived: a gain after its fader (post-fader sends and the outputs follow it,
pre-fader sends do not), fading in and out without a plan swap. The UI proposes
−∞, 100 ms and 500 ms when AFV is switched on. The console lights a strip red on
air and green on preview. `GET /api/v1/tally` lists the lamps and the strips.

## Control room

The control room (§10.1) is part of the show (`control_room`); who is soloed is not
(it starts empty).

- **Monitors** (speakers or headphones, any format): a source (a strip at its
  post-pan, a bus post-fader, or `{"input": n}`: an input's channels 1 and 2),
  `level_db` (−80 … +12), `dim` with `dim_db` (default −21), `cut`, a `check` of L
  and R (`stereo`, `mono` and `difference` with the stereo-to-mono sum, `swap`,
  `left` or `right` on both sides), `follows_solo`, room alignment (`trim_db`,
  `delay_ms`, a `chain` of processing nodes for EQ), a
  `calibration_spl` (dB SPL at 0 dB) for the SPL presets `reference_spl` (79, 82,
  85), and `publish`: an MXL flow with an NMOS sender "Monitor <name>" for physical
  speakers. Every control ramps; at most 16 monitors.
- **Bass management** (formats with an LFE channel), switched on and off per
  monitor: `bass_management` `{"enabled", "crossover_hz" (40–250), "lfe_gain_db"
  (−20 … +20)}`. The other channels are high-passed and their sum low-passed into
  the LFE channel (Linkwitz-Riley 24 dB/oct, flat summed), the LFE input at its
  gain. The UI proposes 80 Hz and +10 dB.
- **Solo:** PFL (pre-fader) and AFL (strips post-pan, buses post-fader) on any strip
  or bus, summed to stereo (PFL with `pfl_trim_db`); `additive` or `interlock`.
  While anything is soloed, a monitor that follows solo plays the solo bus with its
  programme at `programme_db` (default −21 dB; `-inf`: solo replaces it).
  Solo-in-place (`sip`, off by default, destructive) mutes every other strip in
  the mix that is not `solo_safe`.
- **Source buttons** (`sources`): the buttons every monitor shows.
- **Oscillator** (§10.2): a bus with an `oscillator` node; it goes anywhere a bus
  goes (sends, monitors, outputs).

The UI's Control room tab has all of it; PFL and AFL are on the strips.

## Talkback

Talkback (§10.2) is part of the control room (`control_room.talkback`); who talks
is not saved.

- **Mics:** an MXL input channel (`input`, `channel`), with `level_db` (−40 …
  +20), `priority` (0 … 9) and `dims_control_room` (default true): while it talks,
  every control-room monitor is dimmed by its `talkback_dim_db` (default −21 dB).
  A browser's microphone is a mic too (`browser-<n>`, priority
  `browser_priority`, never dims): the browser publishes it by WHIP to MediaMTX
  as `<PREVIEW_PATH_PREFIX>/talkback-<client>`, the mixer reads it over RTSP,
  decodes the Opus to mono and plays it through a jitter buffer (from 20 ms,
  growing by 10 ms on an underrun, at most 100 ms; its delay is in
  `GET /api/v1/talkback`). Browsers give the microphone to HTTPS pages and
  `localhost` only. The built-in MediaMTX takes publishing from the network on
  these paths only.
- **Destinations:** a bus each (studio, IFB/N-1, remote, aux): talk is mixed in
  after the bus fader at `level_db`. `mode` `mix` adds it to the programme;
  `interrupt` (IFB) takes the programme down to `interrupt_db` (default `-inf`,
  cut) while anything talks to it, and only the highest-priority talking mics are
  heard. A `return` (a signal or an input) is what comes back, for listen-back.
- **Talk** is a smoothed value change (10 ms), never a plan swap: latched, or
  momentary (repeated by the client at least every 3 s, else it ends).
- **Listen-back:** the monitors that follow solo play a destination's return (or
  any source) instead of their own.
- A strip channel can be patched to a talkback mic (`{"talkback": "<mic id>"}`),
  a browser's too.

The UI's Control room tab has the talkback panel: the mic (a configured one or
this browser's microphone), a button per destination (a short press latches, a
long one talks while held), "All", listen-back, and the mics and destinations.

## Listening in the browser

Every monitor can be heard in the browser (§10.1, the preview contract of
Appendix B.1): the mixer encodes it as Opus (stereo, 10 ms frames, 128 kbit/s;
other formats downmixed) and publishes it over RTSP to MediaMTX as
`<PREVIEW_PATH_PREFIX>/<monitor id>`; the browser plays it by WHEP.

- **MediaMTX:** without `PREVIEW_PUBLISH_URL` the mixer runs its own as a child
  process (`MEDIAMTX_BIN`; in the image), restarted with backoff (1 s, doubling to
  30 s) when it ends: RTSP on 127.0.0.1:`MEDIAMTX_RTSP_PORT`, WHEP on
  `MEDIAMTX_WHEP_PORT`, ICE on UDP `MEDIAMTX_ICE_UDP_PORT`, `NMOS_HOST_ADDRESS` as
  the ICE address; no API, HLS, RTMP, SRT or MoQ; only the mixer's paths;
  publishing only from this host. Its configuration is written to
  `MIXER_STATE_DIR/mediamtx.yml`. With `PREVIEW_PUBLISH_URL` the streams go to the
  platform's MediaMTX and `PREVIEW_WHEP_URL` is the browser's base.
- **On demand:** a stream is encoded only while someone listens: the UI repeats
  `POST /api/v1/preview/<id>/listen` every 5 s while playing; with
  `MEDIAMTX_API_URL`, readers on MediaMTX count too. 10 s after the last, it stops.
- **Personal monitors:** each browser can make its own (`MIXER_MAX_WEB_MONITORS`,
  default 8): "My headphones" in the Control room tab, with its own source, level,
  dim, cut, check and solo, not part of the show. Unheard for 10 minutes, it is
  removed.
- **Latency:** from the media time of a sample to the browser's audio output,
  79–99 ms (median per run; single pulses 75–104 ms) measured in headless Chrome
  on the development machine (target ≤ 150 ms; `tests/integration/preview.sh`
  with the `second-pulse` oscillator).

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
the `SYS_NICE` capability. The DSP thread runs on the last of the pod's CPUs and
needs its whole physical core: with SMT, the static CPU manager's
`full-pcpus-only` (or SMT off). The mixer keeps its other threads (the meters,
the API, the preview) off that core when it has CPUs on another one;
`dsp.scheduling` in `GET /api/v1/status` says what the core is like, and a
warning is logged when its sibling is outside the pod's CPUs.

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
| `WIDGET_FRAME_ANCESTORS` | 'self' | CSP frame-ancestors of the /widget pages; /widgets answers these origins with CORS. |
| `TSL_ENABLE` | false | Listen for TSL UMD 5.0 tally (audio-follows-video). |
| `TSL_UDP_PORT` | 8910 | TSL UMD 5.0 over UDP. |
| `TSL_TCP_PORT` | 8911 | TSL UMD 5.0 over TCP (DLE/STX framing). |
| `TSL_SCREEN` | -1 | Only this TSL screen; -1 is every screen. |
| `TSL_MAP` | empty | display:index pairs, e.g. 0:1,1:2: the tally index a TSL display index drives; empty: the same number. |
| `PREVIEW_PUBLISH_URL` | unset | RTSP base URL of the shared MediaMTX. Unset: start a built-in MediaMTX and publish there. |
| `PREVIEW_PATH_PREFIX` | mxl-audio-mixer | Path prefix of every stream: <prefix>/<monitor id>. |
| `PREVIEW_WHEP_URL` | unset | Public base URL of WHEP and WHIP for the browser. Unset with the built-in MediaMTX: the page's host and MEDIAMTX_WHEP_PORT. |
| `PREVIEW_HLS_URL` | unset | Public base URL of HLS (listed for the platform; the mixer plays by WHEP). |
| `MEDIAMTX_API_URL` | unset | API of the shared MediaMTX, for reader counts. |
| `MEDIAMTX_RTSP_PORT` | 8954 | RTSP port of the built-in MediaMTX (127.0.0.1). |
| `MEDIAMTX_WHEP_PORT` | 8599 | WHEP and WHIP port of the built-in MediaMTX. |
| `MEDIAMTX_ICE_UDP_PORT` | 8199 | WebRTC ICE UDP port of the built-in MediaMTX. |
| `MEDIAMTX_BIN` | mediamtx | The built-in MediaMTX executable (the image has it). |
| `MIXER_MAX_WEB_MONITORS` | 8 | Most personal monitor paths of browser clients at once. |

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
  `patch_state`. `trim_db` ±42, `fader_db` `"-inf"` … +15, `pan` −1 … +1 (mono:
  position; stereo: balance), `width` 0 … 2 (stereo; 1 unchanged), `loudness`
  (measure it). A mono strip is stereo after its panner.
- **Bus:** sum of its sends → `chain` → fader (`"-inf"` … +15) and mute; `loudness`;
  `compensate` (delay compensation, default `true`). A **mix-minus** bus (N-1, §5)
  has `minus: {"of": "<bus>", "without": [ids]}`: besides its own sends it takes
  every send into `of`, as it is (tap, level, mute, phase), except those from
  `without`; its own send from a source wins. It follows `of` without a plan swap.
  `without` names `of`'s direct sources.
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
- **Tally:** `tally_index` and `afv` on strips (below, "Tally and
  audio-follows-video").
- **Console:** `colour` (`#rrggbb`), `recall_safe`, `solo_safe` and `mute_safe`
  on strips and buses;
  `layouts`: `[{"name", "pages": [{"name", "signals": [ids]}]}]`.
- **VCAs** (§9, `vcas`): `{"id", "name", "fader_db", "mute", "members",
  "colour", "recall_safe"}`; members are strips, buses or other VCAs (nested; a
  cycle is refused). A VCA sums no audio: a member's fader gets the `fader_db`
  (`"-inf"` … +15) of every VCA above it added (each counted once; one at `-inf`
  silences it; fader and VCAs together at most +15 dB), and a muted VCA mutes its
  members without changing their own `mute`. PFL or AFL of a VCA (`PUT
  /api/v1/solo/<vca>`) is that of all its members. VCA ids share the signals'
  namespace; layout pages (and so the `fader-bank` widget) take VCAs too. At most
  64.
- **Mute groups** (`mute_groups`): `{"id", "name", "active", "members"}` (strips
  and buses); while active, every member is muted, again without changing its own
  `mute`. A `mute_safe` strip or bus is never muted by a mute group or a VCA. At
  most 32. Neither VCAs nor mute groups change the plan: only fader values move.
  Deleting a strip, bus or VCA takes it out of VCAs, mute groups and layout pages.
- **Snapshots** (§12.5): parameter states of every strip, bus, send and VCA (trim,
  polarity, fader, mute, pan, width, every chain node's parameters and bypass,
  send gain, mute and phase; VCA fader and mute), and the auto setup as a whole
  (VAD settings, auto-mixes, ducks, auto-levels: a moderation, then a panel
  discussion). A
  recall takes `signals`, `sections` (`strips`, `buses`, `vcas`, `auto`) and
  `classes` (`trim`, `fader`, `mute`, `pan`, `chain`, `sends`, `auto`), all
  optional (everything), and `crossfade_ms` (the ramp of every value); it never
  touches recall-safe signals, auto-mixes, ducks and auto-levels, nor signals, sends and nodes
  that no longer exist (or whose node type changed; a recalled auto-mix or duck
  drops the signals that are gone). The auto setup comes back when both its
  section and its class are wanted; a signal filter does not narrow it.
  Snapshots and layouts are part of the show.
- **Show history:** `MIXER_STATE_DIR/history/`: an automatic version at most once a
  minute after a change (the newest 100 kept), named versions on request (kept
  until deleted). A restore or an import keeps the current show as a version first.

Parameter changes ramp over `ramp_ms` (default 10 ms). A change that rewires the
graph fades the affected paths out and in (10 ms) around the plan swap; unrelated
paths are not touched, and processing nodes keep their state. Parameters are
addressed by signal id and field (§12.4), e.g. `strips/vox/fader_db`,
`sends/vox/main/gain_db`, `strips/vox/chain/eq-1/bands/0/gain_db`.

### Operations (§5)

High-level operations make ordinary buses and sends (editable afterwards), each
`POST /api/v1/operations/<op>` answering `{"buses", "strips", "removed"}`:
- `group` `{"strips", "id"?, "name"?, "format"? (stereo)}`: a subgroup bus; the
  strips' post-fader and post-pan sends move to it and it sends where they sent
  (pre-fader sends stay). Refused while a mix-minus of one of those buses leaves
  out one of the strips (through the group it would be back in).
- `n-1` `{"strips", "of"?, "mode"?: "per-participant" | "one", "format"?}`: with
  `of`, mix-minus buses of it: `n1-<strip>` without that strip, or one without the
  selection; without `of`, a bus per participant with sends from the others. The
  format is `of`'s or mono.
- `mix-minus` `{"of", "without", "id"?, "name"?}`: a mix-minus bus.
- `link` `{"strips", "format"?, "id"?, "name"?}`: mono strips into one stereo or
  surround strip (2: stereo, 3: 3.0, 6: 5.1, 8: 7.1, 10: 5.1.4, 12: 7.1.4): their
  patches in order, the first strip's processing, fader and settings, a send to
  every bus one of them sent to (the first one's); references (layouts, VCAs, mute
  groups, monitor sources, mix-minus) move to it.
- `unlink` `{"strip"}`: a stereo strip into `<id>-l` and `<id>-r`, hard left and
  right, each with its processing, fader and sends.

The console's Operations panel runs them on a selection of strips.

### Voice detection, auto-level, auto-mix and ducking (§8)

The control side runs them every 10 ms; they drive one gain per strip or bus
and channel (its *auto gain*: auto-level × auto-mix × each channel's deepest
duck): where its chain has an `auto` node (placed like any processing node, one
per chain), else after the chain, before the fader. Pre-fader sends carry it;
the operator's fader multiplies with it. They are part of the mix (`vad`,
`automixes`, `ducks`, `autolevels`), the show file and snapshots (section
`auto`); they change gains, never the plan (adding or removing a member, target
or auto-level does). Every analysed signal is tapped at its auto gain's input,
all channels (`vad_drops_total` counts blocks the control side did not get),
and measured there: BS.1770 loudness over its channels (momentary 400 ms,
short-term 3 s, a fast 50 ms reading), the VAD on the mean of its channels.

- **Voice activity detection** analyses a signal in 10 ms frames: the 250 Hz–4 kHz band's power over its noise
  floor (minimum over 2 s), voicing (a pitch of 80–400 Hz by autocorrelation),
  the modulation of syllables, and the rejection of pure tones (line-up, idents,
  sweeps): the `features` detector. By default (`detector: "silero"`) it runs
  Silero VAD, a small neural network (as Com-IS-Assist does), on the signal
  decimated to 16 kHz in 32 ms chunks: it does not mistake music for speech
  (measured: piano and orchestra 0.00 of the time, the feature detector 0.69
  and 0.23), at about 0.8 % of a core per signal; without ONNX Runtime it falls
  back to the feature detector. Speech starts above `threshold` (default 0.5) and ends `hold_ms`
  (800: a pause between sentences is still talking) after the probability fell
  below it (minus 0.15). While the signal's momentary loudness is below
  `gate_lufs` (−32), nothing is speech: crosstalk from other
  talkers is speech too, but quieter (set it 20–30 dB below the talker). A voiced
  onset is detected after about 50–70 ms. Its state per signal:
  `speech_active`, `speech_probability`, `speech_level_dbfs` (the mean of the
  speech frames, 1 s), `level_dbfs`, `momentary_lufs`, `short_term_lufs`,
  `gated`, onsets and
  offsets, and the frame's features. Hold and gate after Com-IS-Assist's
  measurements on commentary. Members of an auto-mix, keys of a duck, targets
  of a ratio duck that hears them, and auto-levelled signals are analysed with
  the defaults unless they have settings of their own; at most 64 signals.
- **Auto-level** (`autolevels`, per strip or bus): the gain rides the signal's
  short-term loudness to `target_lufs` (−23), `speech_offset_lu` (+2) higher
  while its VAD reports speech, within `min_gain_db` … `max_gain_db` (−12 …
  +12), as a one-pole in dB: `attack_ms` (1000) down, `release_ms` (3000) up.
  While the momentary loudness is below `freeze_lufs` (−50) there is no signal:
  the gain holds, and the pause (also the 400 ms that led to the freeze) is not
  measured. Feed-forward: it reads the signal before its gain.
- **Auto-mix** (gain sharing after Dugan) over strips: each member's gain is
  √(its weighted level / the sum), so one talker is at 0 dB, two equal ones at
  −3 dB each, and N members with equal noise at −10·log10(N) dB (the NOM
  attenuation is part of it). Levels: the frame's mean square, falling with
  100 ms. Per member `weight_db` (−30 … +10) and `bypass` (0 dB, out of the
  sharing); muted members (their mute, a VCA, a mute group, the fader at −∞)
  leave the sharing. `last_mic_hold` (default on): while no member speaks, the
  gains stay. `vad_weighting`: a member without speech counts 10 dB less. A
  strip is in at most one auto-mix.
- **Ducks**: `targets` (strips, buses) go down while one of the `keys` speaks.
  `mode: "fixed"` (the default): by `depth_db` (default −12, down to `-inf`),
  linear in dB over `attack_ms` (50), back over `release_ms` (1000) after
  `hold_ms` (800). `mode: "ratio"`: Com-IS-Assist's control (its code, MIT): as
  far down as keeps the keys' short-term loudness `ratio.target_lu` (5) above
  the targets'; `ratio.overvoice_lu` (13) while a target speaks too; when only
  a target speaks (an interview in the bed) back four times faster and without
  the ease-in that otherwise spans a pause between sentences; at most
  `ratio.max_reduction_db` (24); `ratio.speed` (1) scales every stage. Three
  stages, each `target − (keys − targets)` at its time scale: the bed's
  short-term loudness (2.5 s attack), its momentary loudness (±6 dB on top) and
  its fast reading (up to 4 dB more), all against the keys' short-term
  loudness, measured dry (several keys or targets count as one, their powers
  added; an auto-level's gain counted). `ratio.bed_voice: false` ignores the
  targets' speech (and stops analysing them). `channel_depth` (both modes):
  the share of the dB each channel group of a target takes, 0 … 1: `front`
  (L, R), `center` (C, and a mono target), `lfe`, `surround` (every other
  channel). A target of several ducks takes the deepest per channel; a key
  cannot be its own target. A duck from the presenter to the guests is speech
  priority.

The console's Auto panel edits them, with the live speech probability, the
auto-levels' gain and level, the ducks' state (a ratio duck's state, reduction
and ratio held) and each member's gain; strips show a voice dot (green:
speech) and their auto gain (its deepest channel).

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
| `oscillator` | `waveform` `sine` \| `ebu-ident` (EBU R 49: the left channel off for 0.25 s every 3 s) \| `white` \| `pink` (noise, independent on every channel) \| `sweep` (logarithmic, `freq_hz` to `sweep_to_hz` in `sweep_s`, 0.1–600 s, repeated) \| `second-pulse` (the tone in the first 20 ms of every TAI second), `freq_hz`, `level_dbfs` (−100 … 0; peak, for noise RMS); replaces the signal (bypass: the input again) |
| `auto` | no parameters: where the auto gain (auto-level, auto-mix and ducking, above) acts, at most one per chain; bypass: none |

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
or −20 dBFS). These meters are measured on a thread of their own from blocks the
DSP thread hands over (a ring; a block it has no room for is counted in
`meter_drops_total`), not on the DSP thread; loudness is measured on the DSP
thread, block by block.

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
| `TSL_UDP_PORT` (udp) | 8910 | TSL UMD 5.0 tally, only with `TSL_ENABLE=true` |
| `TSL_TCP_PORT` (tcp) | 8911 | TSL UMD 5.0 tally (DLE/STX framing), only with `TSL_ENABLE=true` |
| `MEDIAMTX_RTSP_PORT` | 8954 | the built-in MediaMTX's RTSP, on 127.0.0.1 (without `PREVIEW_PUBLISH_URL`) |
| `MEDIAMTX_WHEP_PORT` | 8599 | the built-in MediaMTX's WHEP (without `PREVIEW_PUBLISH_URL`) |
| `MEDIAMTX_ICE_UDP_PORT` (udp) | 8199 | the built-in MediaMTX's WebRTC media (without `PREVIEW_PUBLISH_URL`) |

| Exit code | Meaning |
| --- | --- |
| 0 | stopped by Ctrl-C |
| 75 | a port cannot be bound (TSL ports with `TSL_ENABLE=true`, the built-in MediaMTX's ports without `PREVIEW_PUBLISH_URL`), or the DSP thread cannot start |
| 78 | invalid configuration, libmxl missing, output domain id clash |
| 143 | SIGTERM: media stopped, MXL released, node deregistered, own domain removed with `MXL_CLEANUP_ON_EXIT=true`, within `SHUTDOWN_TIMEOUT_S` |

## API

| Method and path | |
| --- | --- |
| `GET /` | the web UI |
| `GET /widgets`, `GET /widget/<id>` | operator-screen widgets (above) |
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
| `GET`, `PATCH`, `DELETE /api/v1/strips/<id>` | any strip field (`tally_index` and `afv` take `null` to clear), `ramp_ms`; delete removes its sends |
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
| `GET`, `PATCH /api/v1/control-room` | the control room (with `soloed`); change `sources`, `solo` (`mode`, `pfl_trim_db`, `programme_db`, `sip`), `reference_spl` |
| `POST /api/v1/operations/{group,n-1,mix-minus,link,unlink}` | the high-level operations (above) |
| `GET`, `POST /api/v1/vcas`, `GET`, `PATCH`, `DELETE /api/v1/vcas/<id>` | VCAs (every field, `ramp_ms`; GET adds `signals`: the strips and buses under it) |
| `GET`, `POST /api/v1/mute-groups`, `GET`, `PATCH`, `DELETE /api/v1/mute-groups/<id>` | mute groups (`active`, `members`, `name`) |
| `GET /api/v1/vad`, `GET`, `PUT`, `DELETE /api/v1/vad/<signal>` | every analysed signal: settings (`own`: its own, else the defaults) and `state`; PUT sets its own (`threshold`, `hold_ms`, `gate_lufs`, `detector`: `silero` or `features`) and analyses it; DELETE removes them |
| `GET`, `POST /api/v1/automixes`, `GET`, `PATCH`, `DELETE /api/v1/automixes/<id>` | auto-mixes (`name`, `enabled`, `last_mic_hold`, `vad_weighting`, `members`: strip ids, which keep a member's settings, or `{signal, weight_db, bypass}`); GET adds `gains_db` |
| `GET`, `POST /api/v1/ducks`, `GET`, `PATCH`, `DELETE /api/v1/ducks/<id>` | ducks (`name`, `enabled`, `keys`, `targets`, `mode`, `depth_db`, `attack_ms`, `hold_ms`, `release_ms`, `ratio` {`target_lu`, `overvoice_lu`, `max_reduction_db`, `speed`, `bed_voice`}, `channel_depth` {`front`, `center`, `lfe`, `surround`}, `recall_safe`; the fields of `ratio` and `channel_depth` given are set, the others kept); GET adds `active`, `gain_db` and for a ratio duck `control` (`state`: `release`, `duck`, `interview`, `overvoice`; `reduction_db`, `slow_db`, `mid_db`, `fast_db`, `target_lu`, `ratio_lu`, `dialogue_lufs`, `bed_lufs`) |
| `GET /api/v1/autolevels`, `GET`, `PUT`, `DELETE /api/v1/autolevels/<signal>` | auto-levels (`enabled`, `target_lufs`, `speech_offset_lu`, `max_gain_db`, `min_gain_db`, `attack_ms`, `release_ms`, `freeze_lufs`, `recall_safe`; PUT sets the fields given, the others kept or the defaults); GET adds `state` (`gain_db`, `level_lufs`, `target_lufs`, `frozen`, `speech`) |
| `GET`, `POST /api/v1/monitors`, `GET`, `PATCH`, `DELETE /api/v1/monitors/<id>` | monitors (`format` required to create; every field, `ramp_ms`) |
| `GET`, `DELETE /api/v1/solo`, `PUT`, `DELETE /api/v1/solo/<signal>` | who is soloed; solo a signal (`{"kind": "pfl" \| "afl"}`), end one or every solo |
| `GET /api/v1/preview` | the preview: `mode` (`own`, `shared`), the built-in MediaMTX (`child`: running, pid, restarts, last exit), `whep_url` or `whep_port`, every stream (path, state, frames, connects, error, readers, last listen) |
| `POST /api/v1/preview/<id>/listen` | someone listens to monitor `<id>` (repeat it at least every 10 s); answers `{"path", "whep_url", "whep_port"}` |
| `GET`, `POST /api/v1/personal` | personal monitors; create one (`{"source"?}`; 409 above `MIXER_MAX_WEB_MONITORS`) |
| `GET`, `PATCH`, `DELETE /api/v1/personal/<id>` | one: `source`, `level_db`, `dim`, `cut`, `check` |
| `PUT`, `DELETE /api/v1/personal/<id>/solo/<signal>` | its own solo (`{"kind"}`) |
| `GET`, `PATCH /api/v1/talkback` | mics, destinations, `browser_priority`, the browser mics (state, packets, lost, delay, underruns), who talks, listen-back; PATCH `{"browser_priority"}` |
| `GET`, `POST /api/v1/talkback/mics`, `GET`, `PATCH`, `DELETE …/mics/<id>` | talkback mics (`input` and `channel` required to create) |
| `GET`, `POST /api/v1/talkback/destinations`, `GET`, `PATCH`, `DELETE …/destinations/<id>` | destinations (`bus` required to create; `return` takes `null` to clear) |
| `PUT`, `DELETE /api/v1/talkback/talk/<mic>/<destination>` | talk (`{"momentary": true}`: repeat within 3 s) or stop |
| `DELETE /api/v1/talkback/talk/<mic>`, `DELETE /api/v1/talkback/talk` | stop a mic, or every mic |
| `POST /api/v1/talkback/browser`, `DELETE …/browser/<id>` | `{"client"}`: this browser's microphone is on (repeat within 30 s); answers `{"id", "path", "whip_url", "whip_port"}` |
| `GET`, `PUT`, `DELETE /api/v1/listen-back` | `{"destination"}` (its return) or `{"source"}` |
| `GET /api/v1/tally` | the TSL settings, packets, every tally index's lamps (screen, display, LH, RH, label, age) and the strips that follow one (on air, preview, AFV open) |
| `GET /api/v1/inputs`, `GET /api/v1/inputs/<n>` | state (`unrouted`, `waiting`, `running`, `stalled`), flow, channels, delays, arrival lag, late/missing samples, re-opens |
| `PATCH /api/v1/inputs/<n>` | `{"label": …, "extra_delay": "auto" \| ms}` |
| `POST /api/v1/inputs/<n>/delay/confirm` | apply a proposed automatic delay |
| `/x-nmos/…` on `NMOS_PORT` | IS-04 v1.3, IS-05 v1.2, BCP-007-03 ([mxl-nmos](https://github.com/LeeO86/mxl-nmos) v0.1.1) |

### Live updates

`/api/v1/events` sends JSON messages: `{"type": "status", "status": …}` five times a
second (as `GET /api/v1/status`); `{"type": "mix", "revision", "mix", "latency_ms"}`
on connect and within 40 ms of every change of the mix, from any client; `{"type": "meters",
"meters", "loudness", "vad", "auto_db", "autolevels", "ducks"}` every 40 ms (`vad`: per analysed signal
`active` and `probability`; `auto_db`: every auto gain, its deepest channel, `null` for −∞;
`autolevels`: every auto-level's `state`; `ducks`: every ratio duck's `control`); and `{"type": "analysis", …}` for clients that
asked. A client may send `{"type": "subscribe", "meters": [ids] | null, "meter_hz":
25, "analysis": false}` to receive only some signals' meters, less often, or the
analyser. Changes go through the REST API; every change answers with
`x-mix-revision`, the mix's revision after it.

## Metrics

`dsp_block_seconds` (histogram), `dsp_deadline_misses_total`, `dsp_load_ratio`,
`plan_swaps_total`, `plan_compile_seconds` (last compile), `meter_drops_total`
(metered blocks the meter thread did not get), `vad_drops_total` (tapped blocks
the control engine did not get), `signals`,
`nodes{type}` (steps of the running plan), `output_blocks_total`,
`output_commit_lag_ms{output}`, `loudness_lufs{signal,window}` (window
`momentary`, `short-term`, `integrated`), `true_peak_dbtp{signal}`,
`timing_trusted`, `tally_packets_total`, `vad_active{signal}`, `preview_state{path}` (0 idle, 1
connecting, 2 publishing), `preview_readers{path}`, `preview_child_restarts_total`,
and per input: `input_state`,
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
checked on the API and on the other client; tally and AFV; a toggle pressed twice
around a late mix; listening to a monitor with `MEDIAMTX_BIN`; talkback latched,
held and listen-back; a send drawn and deleted in the graph),
`tests/integration/room.sh` (a published monitor sample-exact with level, dim,
cut and checks; PFL, AFL, interlock, solo-in-place; an input source; the
oscillator with pink noise and a sweep), `tests/integration/tally.sh` (TSL over UDP and TCP, screen filter, AFV open and
closed sample-exact, its fade), `tests/integration/widgets.sh` (the widget contract
and a framing check in headless Chrome from an allowed and a refused origin),
`tests/integration/preview.sh` (MediaMTX built in and shared, monitors played over
WHEP in headless Chrome, the latency, on-demand streams, the child restarted,
personal monitors), `tests/integration/n1.sh` (N-1 per participant and of a
selection by correlation, a mix-minus following its bus without a plan swap, link
and unlink sample-exact, a group), `tests/integration/vca.sh` (VCAs sample-exact: offsets,
nesting, VCA mute, VCA PFL, a mute group with a mute-safe strip, a snapshot of the
VCAs, a cycle refused, no plan swap), `tests/integration/talkback.sh` (talkback into an IFB bus
sample-exact: interrupt, priority, mix, the control room's talkback dim, a
momentary talk that ends, listen-back, a strip on a talkback mic; a browser's
microphone by WHIP from headless Chrome into an MXL output, with its latency),
`tests/integration/auto.sh` (gain sharing sample-exact on two tones with a weight,
bypass, a muted member, without a plan swap; the VAD on espeak-ng speech and not on
a tone; a tone ducked under the speech, its depth, onset latency and release on the
output; two talkers taking turns with last-mic-hold), `tests/perf/section16.sh` (the
§16-shaped load), `ci/amwa.sh off` (AMWA IS-04-01, IS-05-01,
IS-05-02, BCP-007-03-01 against the mixer with a published bus). Hardware tests for
the maintainers are in `tests/hw/`.

`mxl-mixer-tone` (a test tone writer; `--wav` writes a recording instead, e.g.
speech) and `mxl-mixer-probe` (a reader that checks a flow against the tone) are in
the image too.

## Licence

MIT ([`LICENSE`](LICENSE)). Dependencies: `mxl` / `mxl-sys` (Apache-2.0,
dmf-mxl), `mxl-nmos` (MIT OR Apache-2.0), libopus (BSD-3-Clause, linked statically
through `opusic-sys`; its notice is in the image under `/usr/share/doc/libopus`),
ONNX Runtime 1.31 (MIT, Microsoft, linked statically through `ort`, MIT OR
Apache-2.0; its components' notices, among them Eigen under MPL-2.0, are in
[`assets/onnxruntime.ThirdPartyNotices.txt`](assets/onnxruntime.ThirdPartyNotices.txt)
and in the image under `/usr/share/doc/onnxruntime`), the Silero VAD model (MIT,
Silero Team, in the binary; [`assets/silero_vad.LICENSE`](assets/silero_vad.LICENSE)),
Com-IS-Assist's control core (MIT, Andreas Weiss, copied into `src/auto/comis/`
with its notice; [`src/auto/comis/LICENSE`](src/auto/comis/LICENSE), in the
image under `/usr/share/doc/com-is-assist`), and crates under MIT, Apache-2.0, BSD, ISC, Zlib or Unicode-3.0 terms (aws-lc,
through `mxl-nmos`'s HTTP client, is Apache-2.0 and ISC). The image also has
MediaMTX (MIT, a separate executable, `/usr/share/doc/mediamtx`). The UI: Vue
(MIT) and Vue Flow (MIT, the routing graph), built with Vite (MIT); its base styles and components come from the
sibling media functions (MIT, same author).
