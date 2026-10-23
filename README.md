# mxl-audio-mixer

A broadcast audio engine on MXL, in Rust: a freely routable audio matrix, channel
strips, broadcast DSP, loudness, a control room, VCAs and master/slave operation.
The contract is [`SPECIFICATION.md`](SPECIFICATION.md); decisions, deviations and
results are in [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md).

**Status: 1.4.2 released** (2026-10-23; a smoother start of the inputs); 1.4.1
(2026-10-22; the input fixes from test-all: late blocks raise the delay, one read wait
per block, catch-up and re-open fixes); 1.4.0 (2026-10-22; the
operators' feedback on the console, DSP and formats, and views, and the input reading fixes); 1.3.0 (2026-10-17; §19: milestone 9,
the remote link, and the complete logic editor), 1.0.0, 1.1.0 and 1.2.0 before it (milestones 0–8; notes in
`docs/release-notes/`). Settings, probes, metrics, the NMOS node with its
MXL audio receivers, and the inputs read on the TAI block clock (waiting, stalled
and re-created flows, arrival lag and automatic delay); the mix: input patch,
strips (trim, polarity, fader, mute, pan/balance/width), buses, a send matrix with
width mapping, and buses and strips (their direct out) published as MXL flows with NMOS senders; processing
chains (EQ, filters, dynamic EQ, compressor, limiter, expander, gate, de-esser,
ducker, multiband, delay) with delay compensation and block-delay feedback;
peak, peak hold, true peak, RMS, PPM, correlation, gain-reduction and loudness
(BS.1770-4 / R 128) meters and an analyser; snapshots, layouts and the show
history; the web UI (console, processing editor, routing matrix, snapshots,
metering) with live updates over a WebSocket; widgets for operator screens
(`strip`, `fader-bank`, `loudness`, `monitor`, `talkback`, `vca`); tally (TSL UMD 5.0) and
audio-follows-video; the control room (monitors, PFL/AFL, solo-in-place, MXL
monitor outputs, oscillator); talkback and browser monitoring; VCAs, mute groups,
N-1 and group operations; voice detection, auto-level, auto-mix and ducking;
logic graphs; master/slave remote production.

## Web UI

`http://<host>:8190/`: one page built into the binary (no CDN, no other files). Tabs
(the URL hash addresses them): **Console** (strips in banks or in a layout's pages:
patch, trim, polarity, the chain overview with its response and gain reduction,
sends, pan and width, mute, fader, meter; the buses and VCAs beside them, behind a
divider that sets how wide each side is (drag it or use the arrow keys; double-click
or Home: automatic), each side scrolling on its own. A click on a strip's name
renames it, the gear opens its processing, and its ⋮ menu has "Send to…" (a new
send at 0 dB to a bus), on a bus "Add source…" and "Subgroup…", "Format…" (the
format change of the Processing view, confirmed by a second press), and "Delete…" (with
a confirmation listing what goes with it). "+ Strip" and "+ Bus" add several at
once with numbered names; the first new one is selected. A stereo or wider strip's
input patch is linked by default: choose the first channel and the others follow
in order on the same input (untick "Link" to choose each). Wider strips have one
polarity button that opens a toggle per channel and shows how many are inverted.
"Auto-mix" opens the gain-sharing auto-mixes, every strip a tile), **Processing**
(the chain of a strip or bus as tiles of one size in a grid, in chain order: drag
node types in from the palette, which stays in view, drag a tile by its header to
reorder (the wheel scrolls meanwhile, Escape cancels), a click on the header opens
or closes it, bypass, delete; ◀ ▶ or the `[` and `]` keys step to the previous or
next strip or bus; EQ curve with band handles (a double-click resets a band;
high- and low-pass bands with their slope), one band's controls at a time, filter,
dynamics transfer curves with the live level, de-esser listen, keys, the surround
panner's position pad; the signal's format, changed after a confirming second
press, with the nodes it would bypass named first and a flag on every node that
cannot run on the format; a bus's own down-mix), **Routing** (the send matrix,
sources in rows, destination buses in columns: a click on an empty cell marks it
yellow, a second click on it within 3 s sends at 0 dB; a send's tap (only those
its source has), gain, mute, phase and block delay; publishing outputs, a strip as
its direct out, and copying a flow id), **Graph**
(the routing as a graph, read-write: inputs, strips, buses, monitors and outputs
with their patches, sends, mix-minus, monitor sources and outputs; draw from a
strip or bus to a bus to send, click a send to edit it (its tap list offers only
the taps its source has), Delete removes it; nodes
in columns by routing depth, moved ones kept in the browser), **Logic** (logic
graphs, §11: drag blocks in from the palette or click them; draw cables between
ports, coloured by type (boolean, number, level, event, text), where an input that
cannot take the cable is dimmed while drawing; zoom, pan and a minimap; select
several blocks (Ctrl-click, Shift-drag), copy and paste them with their cables
(Ctrl+C, Ctrl+V, also into another graph), Delete; undo and redo (Ctrl+Z,
Ctrl+Shift+Z) of every edit; group blocks into a macro with the ports you choose,
open it (double-click, a breadcrumb back) and ungroup it; a refused edit shows the
server's reasons as red markers on the blocks, ports and the dashed cable; every
block shows its live value; soft keys, triggers, dry run and the list of the last
actions; the list under the canvas edits every parameter, and a click on a block's
row selects it in the graph. A new graph starts in
dry run, and the toolbar shows LIVE, DRY RUN or OFF; undo never changes that. A
new block gets no target by itself (amber until one is chosen: an action without
one never acts, an input without one reads off, 0 or the floor). In a live graph,
paste, group, ungroup, undo and redo ask first when they make actions or blocks
that remember: those actions act at once, and re-made blocks start empty),
**Snapshots** (take, recall with filters, recall-safe, crossfade; layouts; show
history), **Meters** (the bridge on dBFS, true peak or PPM EBU, BBC, Nordic, DIN at
an alignment level, phase correlation; loudness; spectrum and goniometer of any
tap), **NMOS**, **Status**, **Settings** (mix settings with the down-mix, applied
with a button; configuration, show export and import). Every client follows every
change. Faders and pots take touch: drag,
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
| `monitor` | `monitor` (a monitor id) | source buttons, level, dim, mute, mono, the meter | 360×200 |
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
  `level_db` (−80 … +12), `dim` with `dim_db` (default −21), `cut` (the UI's
  Mute), `mute_channels` and `solo_channels` (output channels from 1: muted ones
  are silent; while any is soloed, only the soloed ones play; they act before bass
  management, so a muted LFE still gets the mains' low end; a format change keeps
  the channels the new format has), a `check` of L
  and R (`stereo`, `mono` and `difference` with the stereo-to-mono sum, `swap`,
  `left` or `right` on both sides), `follows_solo`, room alignment (`trim_db`,
  `delay_ms`, a `chain` of processing nodes for EQ), a
  `calibration_spl` (dB SPL at 0 dB) for the SPL presets `reference_spl` (79, 82,
  85), and `publish`: an MXL flow with an NMOS sender "Monitor <name>" for physical
  speakers, and its own `downmix` of its source (`null`: the mix's; the solo bus
  is folded to stereo with the mix's down-mix). Every control ramps; at most 16
  monitors.
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

The UI's Control room tab has all of it (every monitor with Remove, and M and S per
channel); PFL and AFL are on the strips.

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
  dim, cut (Mute), check and solo, not part of the show. With `follows_solo` (off
  by default; "Follow PFL/AFL" on the card) it also plays the control room's
  PFL/AFL like a monitor that follows solo (the programme at `programme_db`
  meanwhile); its own solo, while it has one, wins over the control room's.
  Listen-back stays with the control-room monitors. Unheard for 10 minutes, it is
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
mkdir -p /dev/shm/mxl                              # the scan path must exist
LD_LIBRARY_PATH=/opt/mxl/lib NMOS_HOST_ADDRESS=192.0.2.10 MXL_DOMAIN_SCAN_PATH=/dev/shm/mxl \
    MIXER_STATE_DIR=/tmp/mixer target/release/mxl-audio-mixer
```

The Silero VAD build downloads a prebuilt ONNX Runtime. That download uses the
system certificate store. Behind a TLS-inspecting proxy, install the proxy's CA
in the system store first. To skip the download, fetch the archive for this
target (ort 2.0.0-rc.14, the `x86_64-unknown-linux-gnu` file with no extra
features: `https://cdn.pyke.io/0/pyke:ort-rs/ms@1.31.0/x86_64-unknown-linux-gnu.tar.lzma2`)
with a client that trusts the proxy, and extract it. The file is a raw LZMA2
stream, not a `.tar.xz`:

```sh
xz --format=raw --lzma2=dict=64MiB -dc x86_64-unknown-linux-gnu.tar.lzma2 | tar -x -C /opt/ort
ORT_LIB_LOCATION=/opt/ort cargo build --release
```

`ORT_LIB_LOCATION` is that directory (the one that contains `libonnxruntime.a`).

The image is `ghcr.io/leeo86/mxl-audio-mixer` (uid 1000; tags `X.Y.Z`, `X.Y`, `X`,
`git-<sha7>`, `nightly-dev`). `deploy/k8s/deployment.yaml` shows the platform
conventions; `deploy/grafana/` has a dashboard. The development set-up is in
[`docs/development.md`](docs/development.md).

On the platform: the MXL root at `/Volumes/mxl` (hostPath), a writable `/config`,
the pod IP in `NMOS_HOST_ADDRESS`, the registry in `NMOS_REGISTRY_ADDRESS` /
`NMOS_REGISTRY_PORT`, and the node's kernel TAI offset set (CLOCK_TAI = UTC + 37 s).

**CPUs.** Guaranteed QoS (requests = limits) with an **even** number of whole CPUs
on a node with the static CPU manager: prefer `"4"` (two physical cores); `"2"`
(one core) is the minimum and has not been measured on hardware. The DSP thread
runs on the last of the pod's CPUs whose SMT siblings are all in the pod (a whole
core), so no other workload runs on its sibling; the mixer keeps its other threads
(the meters, the API, the preview) off that core when it has CPUs on another one.
With `"4"` they get a core of their own; with `"2"` the non-DSP threads run on the
DSP CPU's sibling. The kubelet's
`reservedSystemCPUs` should cover the system daemons and the unpinned pods, so
nothing else lands on the pod's cores. `full-pcpus-only` is recommended (it
guarantees whole cores), not required: with an even request the static policy
hands out whole cores when they are free. Check `dsp.scheduling` in
`GET /api/v1/status`: "a core of its own" is right; "outside this process" means
the sibling belongs to another workload (also a warning in the log).

**Real time.** `MIXER_RT_PRIORITY` (≤ 49, e.g. 49) runs the DSP thread
`SCHED_FIFO`. The image gives the binary `CAP_SYS_NICE` as a permitted file
capability; with `SYS_NICE` in the container's capabilities
(`capabilities: {drop: [ALL], add: [SYS_NICE]}`) the mixer raises it at start and
logs `effective capabilities: sys_nice`. Without it the mixer starts anyway, runs
the DSP thread `SCHED_OTHER` and logs a warning; `mxl_audio_mixer_dsp_realtime`
is then 0. The file capability makes glibc run the binary in secure mode:
`LD_LIBRARY_PATH` and `LD_PRELOAD` are ignored (the image finds libmxl through
`ldconfig`, in `/usr/local/lib`).

**Network.** The API has no authentication: keep the web and NMOS ports on the pod
network (the platform's ingress in front of the UI). The logic editor's webhook
block posts only to `http://` and `https://` URLs; a failed post is a warning in
the log (without the URL's credentials, query and fragment). Its HTTP trigger
(`POST /api/v1/logic/<graph>/trigger/<block>`) is part of the same API, so it has
no authentication either: anyone who reaches the web port can fire it. Give an
outside system that should fire one a path through the platform's ingress with
its own authentication, not the web port itself.

## Settings

`MIXER_TEST_LOGIC_SWAP_DELAY_MS` is for `tests/integration/logic.sh` only (it delays
the logic's plan changes and is logged as a warning at start): never set it in
production.

Environment first, then the JSON file `MIXER_CONFIG_FILE` (an object of these keys),
then the defaults. Unknown environment variables are ignored; unknown file keys and
invalid values exit with code 78. `GET /api/v1/config` lists every value with its
origin. One setting is a secret: `MIXER_LINK_TOKEN`. Give it through the
environment from a Kubernetes Secret; `GET /api/v1/config` shows `set`, not the
token.

| Setting | Default | Meaning |
| --- | --- | --- |
| `HOST_ID` | the host name | Instance name; the default node label and the start of the default NMOS seed. |
| `WEB_PORT` | 8190 | Port of the web UI, the API, /livez, /readyz and /metrics. 1.0.0 used 8160. |
| `NMOS_PORT` | 3342 | Port of the IS-04 Node API and the IS-05 Connection API. The next port (3343) is reserved for IS-07 events; not opened in this release. 1.0.0 used 3312. |
| `NMOS_SEED` | <HOST_ID>-mixer | Seed of every NMOS id, the output domain id and the MXL flow ids (UUIDv5). |
| `NMOS_LABEL` | <HOST_ID> | Node label; the Device is "<label> Audio Mixer". |
| `NMOS_TAGS` | {} | JSON object of tag name to string arrays, on the Node and the Device. |
| `NMOS_HOST_ADDRESS` | first non-loopback IPv4 | IPv4 literal announced in NMOS (the pod IP on the platform). |
| `NMOS_REGISTRY_ADDRESS` | unset | IPv4 address or DNS name of the IS-04 Registration API. Unset: no registration. |
| `NMOS_REGISTRY_PORT` | unset | Port of the Registration API (set together with the address). |
| `NMOS_QUERY_ADDRESS` | registry address | IPv4 address or DNS name of the Query API, used for readiness. |
| `NMOS_QUERY_PORT` | registration port + 1 | Query API port. |
| `NMOS_DNS_SD` | false | mDNS/DNS-SD browsing and advertisement (needs a build with the dns-sd feature). |
| `MXL_DOMAIN_SCAN_PATH` | /Volumes/mxl | Parent of the MXL domain directories (an existing absolute directory, else exit 78); input domains (mirrors included) are found below it. |
| `MXL_OUTPUT_DOMAIN_DIR` | <scan path>/mixer-<first 8 hex of the domain id> | The mixer's own output domain, a directory directly below the scan path (else exit 78). Created if missing; the only directory MXL_CLEANUP_ON_EXIT removes. |
| `MXL_OUTPUT_DOMAIN_ID` | UUIDv5 of NMOS_SEED | Id of the own output domain. An existing domain_def.json with another id is kept (a warning), never overwritten. |
| `MXL_HISTORY_DURATION_NS` | 200000000 | history_duration written into a new output domain. |
| `MXL_CLEANUP_ON_EXIT` | false | Remove the own output domain directory on SIGTERM (not a pre-existing domain whose id was kept). |
| `SHUTDOWN_TIMEOUT_S` | 10 | Time for the SIGTERM sequence (1–120). |
| `MIXER_CONFIG_FILE` | unset | JSON file with any of these settings (environment wins). |
| `MIXER_STATE_DIR` | /config | Directory of everything the mixer writes itself (show file, connections); must be writable, else exit 78. |
| `MIXER_LOG_LEVEL` | info | Log level: error, warn, info, debug, trace. |
| `MIXER_INPUT_FLOWS` | 16 | Number of MXL audio inputs (NMOS receivers), 1–256. Every distinct flow read takes an MXL instance, i.e. an inotify instance: `fs.inotify.max_user_instances` (often 128 per uid, shared with every MXL process of that uid) must leave room for them. |
| `MIXER_BLOCK_SAMPLES` | 96 | DSP block size in samples at 48 kHz: 48, 96, 144 or 240. |
| `MIXER_READ_OFFSET` | 2 | Read offset in ms: every input is read at t_out − (offset + its extra delay). |
| `MIXER_OUTPUT_COMMIT_SAMPLES` | one block | Samples per MXL commit of an output (a multiple of the block size). |
| `MIXER_RT_PRIORITY` | off | SCHED_FIFO priority of the DSP thread, 1–49 (needs CAP_SYS_NICE). |
| `MIXER_INPUT_STALL_S` | 2 | An input whose head index does not move for this long is stalled and re-opened. |
| `MIXER_INPUT_DELAY_MAX_MS` | 150 | Upper limit of an input's automatic extra delay. |
| `MIXER_INPUT_DELAY_HOLD_S` | 30 | An automatic delay goes down only after the measured need stayed at least 1 ms lower for this long, and at most once per this time; it goes up at once. |
| `MIXER_INPUT_DELAY_CONFIRM` | false | true: an automatic delay change waits for operator confirmation instead of applying with a fade. |
| `WIDGET_FRAME_ANCESTORS` | 'self' | CSP frame-ancestors of the /widget pages; /widgets answers these origins with CORS. |
| `TSL_ENABLE` | false | Listen for TSL UMD 5.0 tally (audio-follows-video). |
| `TSL_UDP_PORT` | 8910 | TSL UMD 5.0 over UDP. |
| `TSL_TCP_PORT` | 8911 | TSL UMD 5.0 over TCP (DLE/STX framing). |
| `TSL_SCREEN` | -1 | Only this TSL screen; -1 is every screen. |
| `TSL_MAP` | empty | display:index pairs, e.g. 0:1,1:2: the tally index a TSL display index drives; empty: the same number. |
| `TSL_OUT_TARGET` | unset | host:port that the logic editor's tsl-out blocks send TSL UMD 5.0 to over UDP. Unset: no TSL output. |
| `TSL_OUT_SCREEN` | 0 | TSL screen of the logic editor's TSL output, 0–65534. |
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
| `MIXER_LINK_ROLE` | off | Remote link (§13): `off`, `master` or `slave`. |
| `MIXER_LINK_TOKEN` | unset | Pre-shared token (a secret, 8 to 200 bytes). Required when the role is not off. `GET /api/v1/config` shows `set`, not the token. |
| `MIXER_LINK_PEERS` | unset | Master: 1 to 4 `ws://` or `wss://` URLs of slaves (with a host), comma-separated, each slave once. The first is `s1`. |
| `MIXER_LINK_TAKEOVER` | allow | Slave: `allow` or `deny` local changes after the master has disconnected. |

Per input (show file, `PATCH /api/v1/inputs/<n>`): `label` (NMOS receiver label and
group hint, default `Input <n>`) and `extra_delay` (`"auto"`, the default, or ms).
`auto` measures the arrival lag over the last 10 s: for every read block, how far the
flow's head index is behind the end of that block. Blocks in which the writer committed
nothing count too: a writer that commits in batches (a video frame of audio, a burst of
packets) is furthest behind just before its next commit. A head that stands still for
longer than the read offset plus `MIXER_INPUT_DELAY_MAX_MS` is a stall, not a lag. A
block is late exactly when its lag is above the read offset plus the extra delay, so
the arrival lag and `late_samples` agree. The extra delay is the 99.9th percentile plus
1 ms, minus the read offset, within `MIXER_INPUT_DELAY_MAX_MS`. It goes up at once
when the 99.9th percentile comes within 0.5 ms of the delay, and at once when a block
was late (its lag above the read offset plus the delay: a rare outlier the percentile
does not show), to that lag plus 1 ms. It goes down only after the need stayed at
least 1 ms lower for `MIXER_INPUT_DELAY_HOLD_S` (30 s), to the highest need of that
time, and at most once per hold time; the need of a late block counts for one hold
time, and for ten (5 min, renewed each time) once the outlier comes again within
them. The catch-up of a writer after a stall or a re-open is no lag. Lags beyond the
read offset plus `MIXER_INPUT_DELAY_MAX_MS` never move the delay; they are shown, but
the percentiles of a writer always that late read the edge of the measured range
(`lag_ms`, the last lag, is exact). An input starts at the automatic delay its flow
had last (`MIXER_STATE_DIR/input-delays.json`, per flow id), which holds for a hold
time and covers outliers of that size; in its first minute (the start-up phase) a
late block's floor and the period of lower need last 5 s instead of the hold time, so
a backlog at the start (a mirror still filling) does not keep a large delay for
minutes. A change is applied with a
fade, or waits for `POST /api/v1/inputs/<n>/delay/confirm` with
`MIXER_INPUT_DELAY_CONFIRM=true`. A writer that commits N samples at once needs about
N samples of read offset plus extra delay: with the 2 ms default, a browser source that
commits a 50 Hz frame of audio (960 samples) gets about 19 ms of automatic delay.

## The mix

Kept in `MIXER_STATE_DIR/mix.json` and in the show file. `mix.json` is written
atomically: within about 0.6 s of fader and parameter changes (from the API or
from logic, at most every 500 ms; a crash loses at most that much), at once
after any other change, and at SIGTERM. Signal ids (strips and
buses share one namespace) are 1–32 characters `a-z`, `0-9`, `-`; a new one without
an id gets `strip-<n>` or `bus-<n>`.

- **Strip:** patch → trim and polarity → `chain` → fader and mute → panner.
  `patch` has one entry per strip channel: `{"input": n, "channel": c}` (both from
  1) or `null`. A channel the input does not have is silent and shows `missing` in
  `patch_state`. `trim_db` ±42, `fader_db` `"-inf"` … +15, `pan` −1 … +1 (mono:
  position; stereo: balance), `width` 0 … 2 (stereo; 1 unchanged), `loudness`
  (measure it). A mono strip is stereo after its panner; with a surround panner
  (below) a mono or stereo strip is 5.1 or 7.1 there.
- **Bus:** sum of its sends → `chain` → fader (`"-inf"` … +15) and mute; `loudness`;
  `compensate` (delay compensation, default `true`). A **subgroup** bus
  (`subgroup: true`) takes every send into it from its source's output (a strip's
  post-pan, a bus's post-fader) at 0 dB, whatever the send's `tap` and `gain_db`
  say: its fader is the group level. Every change stores its sends so (also a
  recall, an import or the remote link; making a bus a subgroup is ramped), so the
  API shows what is heard; clearing the flag leaves them at 0 dB. A send's `mute`
  and `phase` still apply, and a PUT of another level or tap into it is refused
  (400). The `group` operation makes subgroups. A mix-minus `of` a subgroup takes
  those sends as the subgroup does (0 dB from the source's output). A show with a
  subgroup cannot be loaded by 1.3.0. A **mix-minus** bus (N-1, §5)
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
- **Output:** a bus (post-fader) or a strip (its direct out: post-fader, before
  the pan) published as an `audio/float32` MXL flow in the own domain and an NMOS
  Source, Flow and Sender (label = its name, group hint `<name>:Audio`).
  `format` (default: the signal's) maps it to another layout; `commit_samples`
  (default `MIXER_OUTPUT_COMMIT_SAMPLES`) is a multiple of the block size. The MXL
  flow id is UUIDv5 of `NMOS_SEED` and the signal id, so it survives restarts. At
  most 128 outputs.
- **Formats** (§4.1), channel order as SMPTE ST 2110-30: mono `M`; stereo `L R`;
  `3.0` `L R C`; `5.1` `L R C LFE Ls Rs`; `7.1` `L R C LFE Lss Rss Lrs Rrs`;
  `5.1.4` = 5.1 + `Ltf Rtf Ltr Rtr`; `7.1.4` = 7.1 + the same heights.
- **Width mapping** (sends, monitors, solo and outputs): same channels map 1:1;
  missing ones fold down with the **down-mix** (ITU-R BS.775 style): C → L + R at
  `centre_db`; Ls → L at `surround_db` (or to Lss); Lss, Lrs → Ls, Ltf → L and
  Ltr → Lrs (or Ls) at −3 dB; LFE → L + R at `lfe_db` (`"-inf"`: dropped);
  stereo → mono with the stereo-to-mono sum; mono → C (3.0 and the surround
  formats) or L + R with the pan law (stereo); channels the source lacks stay
  silent. `normalise` scales the whole matrix of a down-mix from 3.0 or wider into
  fewer channels (the front channels too) by its largest row sum, so that no
  output channel sums to more than unity.
- **Down-mix** (`downmix`: `{"centre_db", "surround_db", "lfe_db", "normalise"}`;
  levels −40 … 0 dB, LFE −40 … +10 dB, or `"-inf"`): the mix's in its settings
  (default −3, −3, `"-inf"`, `false`: the fixed down-mix of 1.3), and a bus's or a
  monitor's own, for everything arriving there (`null`: the mix's), the sends a
  mix-minus (N-1) bus mirrors included: they use the N-1 bus's down-mix. The solo
  bus and published layouts use the mix's. A `downmix` in a PATCH replaces the
  whole object (fields left out take their defaults). The mix's down-mix is left out of the
  mix (and `mix.json`) while it is the default, so an unchanged mix loads in 1.3;
  `GET /api/v1/mix/settings` always shows it. A change is not ramped: the
  sends whose coefficients change fade out and in (10 ms, a brief dip), the others
  are not touched. The UI asks for a second press.
- **Format change** (`PATCH` a strip's or bus's `format`): the chain stays; nodes
  that run on any width adapt, nodes that cannot (M/S / width off stereo, a
  surround panner off a mono or stereo strip) are bypassed, flagged in
  `chain_warnings` and pass the signal through; sends stay and use the width
  mapping; a strip's patch is trimmed, or extended (a run of consecutive channels
  of one input goes on, else unpatched) and its polarity with it; a published bus
  keeps its flow's layout (its output gets the old `format`). The answer lists
  what changed in `warnings`. The signal fades out and in.
- **Mix settings:** `pan_law_db` 0, −3 (default), −4.5, −6; `mono_sum_db` −3
  (default) or −6; `loudness_target_lufs` (−23) and `loudness_tolerance_lu` (1);
  `downmix` (above).
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
  Deleting a strip or bus also removes its sends, its output, its mix-minus and
  talkback destinations and references and its auto setup; a monitor, a
  control-room source or a talkback return that listened to it listens to
  nothing; another signal's key on it is cleared (a compressor keys itself, a
  ducker's empty key hears nothing); logic blocks that name it (also a `set-talk`
  to one of its talkback destinations) stay, wired, with an empty target: they
  no longer act on it but still pass their value on; a `set-monitor` that chose
  it chooses no source.
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
- `group` `{"strips", "id"?, "name"?, "format"? (stereo)}`: a subgroup bus (`subgroup: true`); the
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

### Remote production (§13)

The gallery is the master and the far-side mixer is the slave. The gallery console
draws the far-side strips, inputs and auxiliaries as another layer and rides them
over the link (fader, mute, send). The far side mixes its own voices into the
in-ears, so that mix does not wait for the trip to the gallery and back.

`POST /api/v1/remotes/<id>/return` `{"of", "without", "return_strip", "voices"}`
builds that on both sides. On the gallery, `of` (a bus) becomes a published
mix-minus, `ret-<peer>` (so `ret-s1`), without the local strips in `without` — the strips on this
desk that carry the far-side contribution, so those voices are not sent back late.
On the far side, each voice gets an in-ear bus `iem-<voice>`: the other voices,
plus `return_strip` (the far-side strip patched to that published flow). Route the
output to that strip over MXL. Building it again keeps send levels the operator
has set.

`GET /api/v1/remotes` is the layer: each peer (`s1` … `s4`) has `desk` (strips,
buses, sends), `peaks` (dBFS, about 10 Hz), `rtt_ms` and `return_bus`.
`POST /api/v1/remotes/<id>/command` is `{"op":"mute"|"fader"|"send"|"recall", …}`.
A send that does not exist yet is created. `POST /api/v1/remotes/<id>/push` copies
the faders, mutes and sends the gallery last saw. When the master drops, the far
side holds the mix; the console says so.

The link (`/api/v1/link` on the slave's `WEB_PORT`):

- Security: the token is the first message. Over `ws://` it travels in clear,
  so keep a `ws://` link on a trusted network. Across anything else use
  `wss://`: the master speaks TLS (rustls with the system certificate store),
  and the slave serves plain WebSocket, so put a TLS-terminating proxy or
  ingress in front of its web port. For a private CA, install it in the
  master's system store, or point `SSL_CERT_FILE` (a PEM file) or
  `SSL_CERT_DIR` at it; either one replaces the system store. A wrong token is
  refused and logged on the slave (at most every 10 s, with a count). Put the
  token in `MIXER_LINK_TOKEN`, not in the URL: a user or password there is
  refused.
- One master at a time: a second one is refused (its `last_error` is "another
  master is connected") until the first has gone.
- Dead link: each side pings every second and closes the link when it has
  heard nothing for 5 s. The master reconnects every 2 s. `remote_link_up` is
  0 meanwhile, on the master (`peer="s1"`…, listed from the start, so a slave
  never reached shows too) and on the slave (`peer="master"`), and the RTT is
  left out until the link is back.
- Takeover: with `MIXER_LINK_TAKEOVER=deny`, local changes on the slave are
  refused (400) from the moment its master has gone until one is back; the
  master's own commands are never refused. Before the first master, the slave
  loads its show as usual.
- Delivery is at most once: commands are numbered on each connection and the
  slave acknowledges each (a refused one shows in `last_error`). Commands are
  not resent; one that was not sent when the link drops is dropped. After a
  reconnect, push to bring the far side to what the gallery last saw.
  `command`, `push` and `return` answer 409 while the far side is down, and
  `command` 503 when 1024 commands are waiting (a stuck link); the body is
  `{"error"}`.
- A hello must come within 5 s, and a message may have 4 MiB at most.
- A message of a type the other version does not know is ignored (logged once
  per connection), and a command or push it cannot read is refused, so the
  link stays up between versions. A fader at −∞ is `"-inf"` (`null` from
  versions before 1.3.0 is read as −∞).
- The round trip (`rtt_ms`, `remote_link_rtt_ms`) is measured on each side's
  own clock, with its own pings.
- `ret-<peer>` taken by another signal: `return` is refused; rename it.

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
| `eq` | `bands`: 1–8 of `{"type": "bell" \| "low-shelf" \| "high-shelf" \| "notch" \| "high-pass" \| "low-pass", "freq_hz", "q", "gain_db", "slope_db_per_oct"}` (RBJ cookbook; gain ±24 dB, Q 0.1–40; high- and low-pass: Butterworth as the `filter` node, `slope_db_per_oct` 12 / 24 / 36 / 48 required, gain and Q unused) |
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
| `width` | M/S / width, stereo only: `width_pct` 0–200 (0 mono, 100 unchanged, 200 double side), `input` and `output` `lr` \| `ms` (M = (L + R) / 2, S = (L − R) / 2; L = M + S, R = M − S); a gain matrix that ramps per sample, bypass ramps to the identity |
| `surround-pan` | surround panner of a mono or stereo strip: `layout` `5.1` \| `7.1`, `azimuth_deg` −180 … 180 (0 front centre, negative left; a stereo strip's L and R at ∓30° from it), `spread_pct` 0–100 (towards all speakers alike), `divergence_pct` 0–100 (the centre's share into L and R), `lfe_db` −100 … +10 (−100: off; the LFE send is full band, no low-pass); constant power. A stereo strip aimed behind has L and R mirrored (L at the right rear). It replaces the strip's stereo panner, wherever it sits in the chain: the post-pan tap has the layout's channels. At most one per chain; on a bus it does nothing (flagged) |

Ranges: thresholds −100 … 0 dB, attack 0–1000 ms, release 0–10 000 ms, makeup
±24 dB, a chain at most 32 nodes. A node that cannot run on its signal's format
passes the signal through and is listed in the strip's or bus's
`chain_warnings` (`[{"node", "message"}]`). A key is another signal's tap; its signal is
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

### Logic editor (§11)

A logic graph (`/api/v1/logic`, the Logic tab) is blocks wired port to port,
saved in the show. It is evaluated every 10 ms on the control engine (never on
the DSP thread), in topological order. Each graph can be enabled or disabled;
with `dry_run` it computes and lists its actions without running them.

**Ports are typed:** `boolean`, `number`, `level` (dB), `event` (true for one
tick) and `string`. An input converts what it receives: booleans, numbers and
levels into each other (on is ≥ 0.5, true is 1); a value into an event fires when
it turns on (not on the first tick); an event into a value is on for its tick;
anything but an event into a string (numbers with up to two decimals). A string
feeds only a string and an event never feeds a string. An input takes one wire;
one without a wire reads off, 0 or empty. A loop must pass through a block that
remembers (`edge`, `latch`, `toggle`, `counter`, `timer`, `delay`, `debounce`):
those publish last tick's value.

**Blocks** (`GET /api/v1/logic-blocks` lists each with its parameters and ports):
- inputs: `const`, `text`, `button` (a soft key), `trigger` (fired by `POST
  /api/v1/logic/<graph>/trigger/<block>`), `mute`, `pfl`, `fader`, `vad`, `level`,
  `above` (a peak level with hysteresis), `loudness` (momentary, short-term,
  integrated, range or true peak), `tally` (on air), `schedule` (UTC time of
  day), `nmos-receiver` and `nmos-sender` (active), `link` (a remote-link peer is
  up);
- logic: `and`, `or`, `xor`, `not`, `compare` (`gt`, `lt`, `eq`, `ge`, `le`, `ne`),
  `math` (`add`, `sub`, `mul`, `div`, `min`, `max`), `scale` (× factor + offset),
  `map` (a curve through 2–16 points), `select` (one of 2–8 inputs), `edge`,
  `latch`, `toggle`, `counter`, `timer` (a pulse), `delay`, `debounce`;
- actions: `set-mute`, `set-fader` and `set-vca` (with `ramp_ms`), `set-param` (a
  node parameter the DSP smooths, e.g. `threshold_db` or `bands/1/gain_db`, or
  `bypass`; clamped to its range, ramped), `set-pfl`, `set-talk`, `set-publish`,
  `set-monitor` (a monitor's source), `loudness-control` (start, stop, reset),
  `recall` (a snapshot), `webhook` (POST `{"graph", "block", "value"}`), `tsl-out`
  (TSL UMD 5.0 to `TSL_OUT_TARGET`: left lamp red, right lamp green, a label; on
  every change and every second), `log` (a log line and a `logic` message on the
  events WebSocket).

Value actions (mute, faders, parameters, PFL, talk, publish, TSL) follow their
input from the first tick on; event actions run when their input fires. An
action whose input has no wire never runs, also through an unconnected macro
input (a logic block in between is a source of its own: an unconnected macro
input into `not` into `set-mute` still mutes, as an unwired `not` would at the
top level), and neither does one whose target is still empty (`""`: listed in the
graph's `warnings`, "choose a target"). Actions are the same changes the API makes, ramped and clamped the same
way; NaN is no change, +∞ is a fader's maximum. Each action block runs at most
every 20 ms (TSL 50 ms, a webhook 100 ms, changes that rewire the plan 250 ms;
the newest value wins); at most 8 webhook posts run at once. A block keeps its
state over an edit only while its type and parameters stay the same: an action
moved to another target acts on it at once, the old target stays as it is.
`edge`, timers and converted events never fire on the first tick or on a
re-wired input. Disabling and enabling a graph, or a restart, resets toggles,
latches and counters, and value actions apply their input again. A `tsl-out`
index that goes away is switched off once.

**Macros** (`/api/v1/logic-macros`, saved in the show as `logic_macros`) are
sub-graphs used as a `macro` block: their `macro-in` and `macro-out` blocks are
the instance's ports. Evaluation inlines every instance (its blocks are then
`<instance>.<block>` in `values`, `buttons` and the action list), with its own
state. A macro that uses itself is refused, macros nest at most 4 deep, and a
graph has at most 256 blocks once inlined (32 graphs, 64 blocks per graph or
macro, 32 macros).

**Editing:** `PUT /api/v1/logic/<id>` replaces a whole graph, and `POST
/api/v1/logic-batch` writes and deletes graphs and macros as one change. Both
take a `revision` (from GET; it changes with every edit) and answer 409 when the
graph changed meanwhile. A refused change answers 400 with `issues`: each with
its `graph` or `macro`, `block`, `port`, `wire` and `message`.
`POST /api/v1/logic-validate` checks the same body without applying it.

## Ports and exit codes

| Port | Default | |
| --- | --- | --- |
| `WEB_PORT` | 8190 | web UI, API, events WebSocket, probes, metrics |
| `NMOS_PORT` | 3342 | IS-04 Node API, IS-05 Connection API (3343 is reserved for IS-07 events; not opened in this release) |
| `TSL_UDP_PORT` (udp) | 8910 | TSL UMD 5.0 tally, only with `TSL_ENABLE=true` |
| `TSL_TCP_PORT` (tcp) | 8911 | TSL UMD 5.0 tally (DLE/STX framing), only with `TSL_ENABLE=true` |
| `MEDIAMTX_RTSP_PORT` | 8954 | the built-in MediaMTX's RTSP, on 127.0.0.1 (without `PREVIEW_PUBLISH_URL`) |
| `MEDIAMTX_WHEP_PORT` | 8599 | the built-in MediaMTX's WHEP (without `PREVIEW_PUBLISH_URL`) |
| `MEDIAMTX_ICE_UDP_PORT` (udp) | 8199 | the built-in MediaMTX's WebRTC media (without `PREVIEW_PUBLISH_URL`) |

| Exit code | Meaning |
| --- | --- |
| 0 | stopped by Ctrl-C |
| 75 | a port cannot be bound (TSL ports with `TSL_ENABLE=true`, the built-in MediaMTX's ports without `PREVIEW_PUBLISH_URL`), libmxl cannot be loaded, or the DSP thread cannot start. Every port is checked before the output domain is created, and a failed start removes a domain it created. |
| 78 | invalid configuration: an invalid setting, `MXL_DOMAIN_SCAN_PATH` missing or not a directory, `MXL_OUTPUT_DOMAIN_DIR` not directly below it, `MIXER_STATE_DIR` not writable, an unreadable `domain_def.json` |
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
| `GET`, `PATCH /api/v1/mix/settings` | `{"pan_law_db": …, "mono_sum_db": …, "loudness_target_lufs": …, "loudness_tolerance_lu": …, "downmix": …}` |
| `GET`, `POST /api/v1/strips` | list (with `patch_state`), create (`format` required) |
| `GET`, `PATCH`, `DELETE /api/v1/strips/<id>` | any strip field (`tally_index` and `afv` take `null` to clear), `ramp_ms`; a `format` change keeps the chain and sends (above; answer `warnings`); GET adds `chain_warnings`; delete removes its sends and references (above, "Mute groups") |
| `GET`, `POST /api/v1/buses` | list, create (`format` required) |
| `GET`, `PATCH`, `DELETE /api/v1/buses/<id>` | `name`, `format` (as strips), `fader_db`, `mute`, `chain`, `loudness`, `compensate`, `colour` (`""` clears it), `recall_safe`, `subgroup`, `downmix` (`null`: the mix's), `ramp_ms`; GET adds `chain_warnings`; delete removes its sends and output |
| `GET /api/v1/sends` | every send |
| `GET`, `PUT`, `DELETE /api/v1/sends/<from>/<to>` | create or change: `tap`, `gain_db`, `mute`, `phase`, `ramp_ms` |
| `GET /api/v1/outputs`, `GET /api/v1/outputs/<signal>` | published outputs: format, channels, commit size, MXL flow id, NMOS sender id, blocks, commits, errors, commit lag |
| `PUT`, `DELETE /api/v1/outputs/<signal>` | publish a bus or a strip (its direct out) (`{"format": …, "commit_samples": …}`, both optional) or unpublish |
| `GET /api/v1/taps` | the taps each strip and bus offers (for sends, keys, the analyser): `{"<signal>": ["input", …, {"after": "<node>"}]}` |
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
| `GET /api/v1/remotes` | far-side peers (`s1`…): `desk`, `peaks`, `rtt_ms`, `return_bus`, `up` |
| `POST /api/v1/remotes/<id>/command` | `mute`, `fader`, `send` (`from`, `to`, `db`, `mute`), `recall` on that far side |
| `POST /api/v1/remotes/<id>/return` | gallery mix-minus (`of`, `without`) published, and far-side in-ears (`return_strip`, `voices`) |
| `POST /api/v1/remotes/<id>/push` | copy the faders, mutes and sends the gallery last saw |
| `GET`, `POST /api/v1/vcas`, `GET`, `PATCH`, `DELETE /api/v1/vcas/<id>` | VCAs (every field, `ramp_ms`; GET adds `signals`: the strips and buses under it) |
| `GET`, `POST /api/v1/mute-groups`, `GET`, `PATCH`, `DELETE /api/v1/mute-groups/<id>` | mute groups (`active`, `members`, `name`) |
| `GET /api/v1/vad`, `GET`, `PUT`, `DELETE /api/v1/vad/<signal>` | every analysed signal: settings (`own`: its own, else the defaults) and `state`; PUT sets its own (`threshold`, `hold_ms`, `gate_lufs`, `detector`: `silero` or `features`) and analyses it; DELETE removes them |
| `GET`, `POST /api/v1/automixes`, `GET`, `PATCH`, `DELETE /api/v1/automixes/<id>` | auto-mixes (`name`, `enabled`, `last_mic_hold`, `vad_weighting`, `members`: strip ids, which keep a member's settings, or `{signal, weight_db, bypass}`); GET adds `gains_db` |
| `GET`, `POST /api/v1/ducks`, `GET`, `PATCH`, `DELETE /api/v1/ducks/<id>` | ducks (`name`, `enabled`, `keys`, `targets`, `mode`, `depth_db`, `attack_ms`, `hold_ms`, `release_ms`, `ratio` {`target_lu`, `overvoice_lu`, `max_reduction_db`, `speed`, `bed_voice`}, `channel_depth` {`front`, `center`, `lfe`, `surround`}, `recall_safe`; the fields of `ratio` and `channel_depth` given are set, the others kept); GET adds `active`, `gain_db` and for a ratio duck `control` (`state`: `release`, `duck`, `interview`, `overvoice`; `reduction_db`, `slow_db`, `mid_db`, `fast_db`, `target_lu`, `ratio_lu`, `dialogue_lufs`, `bed_lufs`) |
| `GET /api/v1/autolevels`, `GET`, `PUT`, `DELETE /api/v1/autolevels/<signal>` | auto-levels (`enabled`, `target_lufs`, `speech_offset_lu`, `max_gain_db`, `min_gain_db`, `attack_ms`, `release_ms`, `freeze_lufs`, `recall_safe`; PUT sets the fields given, the others kept or the defaults); GET adds `state` (`gain_db`, `level_lufs`, `target_lufs`, `frozen`, `speech`) |
| `GET`, `POST /api/v1/logic`, `GET`, `PUT`, `PATCH`, `DELETE /api/v1/logic/<id>` | logic graphs (§11, above): `name`, `description`, `enabled`, `dry_run`, `blocks`, `wires`; PUT the whole graph; PUT and PATCH take `revision` (409 when stale); answers add `revision`, `ports`, `values`, `buttons` and `actions` (the last 20) |
| `PUT /api/v1/logic/<id>/buttons/<block>` | a soft key (`{"pressed": true}`); not part of the show |
| `POST /api/v1/logic/<id>/trigger/<block>` | fire a `trigger` block on the next tick (204; no body; no authentication, as the rest of the API) |
| `GET`, `POST /api/v1/logic-macros`, `GET`, `PUT`, `PATCH`, `DELETE /api/v1/logic-macros/<id>` | logic macros: `name`, `description`, `blocks`, `wires`; answers add `revision`, `ports` and `exposed` (the instance's ports) |
| `POST /api/v1/logic-batch`, `POST /api/v1/logic-validate` | `{"graphs", "macros", "delete_graphs", "delete_macros"}` as one change (409 `conflicts` on a stale `revision`), or only checked (`{"issues"}`) |
| `GET /api/v1/logic-blocks` | every logic block type: `label`, `category`, `params`, `inputs`, `outputs`, `remembers` |
| `GET`, `POST /api/v1/monitors`, `GET`, `PATCH`, `DELETE /api/v1/monitors/<id>` | monitors (`format` required to create; every field, `downmix` (`null`: the mix's), `ramp_ms`) |
| `GET`, `DELETE /api/v1/solo`, `PUT`, `DELETE /api/v1/solo/<signal>` | who is soloed; solo a signal (`{"kind": "pfl" \| "afl"}`), end one or every solo |
| `GET /api/v1/preview` | the preview: `mode` (`own`, `shared`), the built-in MediaMTX (`child`: running, pid, restarts, last exit), `whep_url` or `whep_port`, every stream (path, state, frames, connects, error, readers, last listen) |
| `POST /api/v1/preview/<id>/listen` | someone listens to monitor `<id>` (repeat it at least every 10 s); answers `{"path", "whep_url", "whep_port"}` |
| `GET`, `POST /api/v1/personal` | personal monitors; create one (`{"source"?, "follows_solo"?}`; 409 above `MIXER_MAX_WEB_MONITORS`) |
| `GET`, `PATCH`, `DELETE /api/v1/personal/<id>` | one: `source`, `level_db`, `dim`, `cut`, `check`, `follows_solo` |
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
`autolevels`: every auto-level's `state`; `ducks`: every ratio duck's `control`);
`{"type": "logic", "graph", "block", "action", "dry_run", "t_ms"}` for every
logic action (with `"log"` from a `log` block); and `{"type": "analysis", …}` for clients that
asked. A client may send `{"type": "subscribe", "meters": [ids] | null, "meter_hz":
25, "analysis": false}` to receive only some signals' meters, less often, or the
analyser. Changes go through the REST API; every change answers with
`x-mix-revision`, the mix's revision after it. The web UI shows a mix only when its
revision is not older than the one it shows, and takes its own logic edits from
their answers at once.

## Metrics

`dsp_block_seconds` (histogram), `dsp_deadline_misses_total`, `dsp_load_ratio`,
`dsp_realtime` (1 when the DSP thread runs `SCHED_FIFO`), `plan_swaps_total`, `plan_compile_seconds` (last compile), `meter_drops_total`
(metered blocks the meter thread did not get), `vad_drops_total` (tapped blocks
the control engine did not get), `signals`,
`nodes{type}` (steps of the running plan), `output_blocks_total`,
`output_commit_lag_ms{output}`, `loudness_lufs{signal,window}` (window
`momentary`, `short-term`, `integrated`), `true_peak_dbtp{signal}`,
`timing_trusted`, `tally_packets_total`, `vad_active{signal}`, `preview_state{path}` (0 idle, 1
connecting, 2 publishing), `preview_readers{path}`, `preview_child_restarts_total`,
and per input: `input_state`,
`input_lag_ms`, `input_arrival_lag_ms` (quantiles 0.5, 0.99, 0.999),
`input_effective_delay_ms`, `input_late_samples_total`
(the source had not written the samples yet), `input_overrun_samples_total`
(the ring had already overwritten them), `input_missing_samples_total`
(both, and any other failed read), `input_reopens_total`; `logic_ticks_total`,
`logic_actions_total{graph}`; with the remote link, `remote_link_up{peer}` and
`remote_link_rtt_ms{peer}`; `process_cpu_seconds_total`,
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
reader, unpublish, loop rejection, a subgroup's locked sends, a deleted bus's
references, a strip's direct out, the taps of a strip and a bus),
`tests/integration/processing.sh` (EQ
sample-exact, ramped parameters without a plan swap, bypass, high-pass, compressor
gain reduction, limiter ceiling and latency, loudness and its controls),
`tests/integration/show.sh` (snapshots with filters, recall-safe and crossfade,
layouts, show history, PPM, correlation, the analyser, the events WebSocket),
`tests/integration/ui.sh` (the UI in headless Chrome with two clients: every action
checked on the API and on the other client; tally and AFV; a toggle pressed twice
around a late mix; the processing tiles (a band reset by a double-click, a tile
opened and closed by its header and dragged while the wheel scrolls, the next and
previous strip); a monitor's Mute, channel mute and solo, Remove; a send by two
clicks in the matrix, a bus's taps, a strip's direct out and its copied flow id;
listening to a monitor with `MEDIAMTX_BIN`; talkback latched,
held and listen-back; a send drawn and deleted in the graph; a logic block
selected from its row; the logic editor,
`web/tests/logic.mjs`: palette, cables, undo and redo, copy and paste, typed ports,
validation markers, macros, dry run; the console's strip handling,
`web/tests/console.mjs`: several strips at once, rename, the linked patch, polarity
of a 5.1 strip, "Send to…" and "Add source…", a subgroup, delete, the divider),
`tests/integration/room.sh` (a published monitor sample-exact with level, dim,
cut, checks and the mute and solo of single channels; PFL on personal monitors
that follow solo or not; PFL, AFL, interlock, solo-in-place; an input source; the
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
output; two talkers taking turns with last-mic-hold), `tests/integration/link.sh`
(two mixers on the remote link: the gallery rides the far side, the return and
the in-ears; a second master, a connection without hello and an oversized
message refused; a hung master and a hung far side dropped within 5 s and
reconnected, takeover deny meanwhile), `tests/integration/logic.sh` (logic
graphs end to end: an HTTP trigger mutes a strip, a soft key ramps its fader
sample-exact, publishes a bus, sends TSL to a UDP listener and posts a webhook, a
log line on the events WebSocket, dry run, a macro written in a batch, a type
error and a stale revision refused, a logic unpublish and an API publish in the same
fade both kept, `mix.json` coalesced and written at SIGTERM), `tests/perf/section16.sh` (the
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
