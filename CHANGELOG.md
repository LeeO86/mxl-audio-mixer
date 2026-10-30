# Changelog

## Unreleased — 1.5.0

- C7: virtual outputs: `MIXER_OUTPUT_FLOWS` / `MIXER_OUTPUTS` fixed MXL flows and NMOS senders (`out-vout:<n>`, stable ids) the designer wires; the operator picks a bus, a direct out, a monitor or an input straight through; silent without a source; `/api/v1/virtual-outputs`, metric `virtual_output_routed`.
- C6: control-room monitors can feed a virtual output, per output either the clean feed (the monitor's source in its format and down-mix, before solo, talkback, checks and level; the default) or the speaker feed (`{"monitor": id, "feed": "speaker"}`).
- C1: the graph's send panel edits the send's surround pan (`Send.pan`, B6) into 5.1 or wider, else the strip's pan (mono or stereo source) and the target bus's down-mix (wider into narrower).
- C2: inputs show the name of what is connected (IS-04 sender label from the Query API, else the MXL flow label) and take an alias, inherited by their strips; virtual and published outputs show what feeds them.
- C3: an input's MXL flow is chosen in the graph (`GET /api/v1/flows`, `PUT /api/v1/inputs/<n>/route` through the own IS-05 receiver).
- C4: double-click in the graph opens a strip's or bus's processing, a monitor's control room.
- C5: the graph keeps zoom, pan and its open panel over a tab switch.
- Downgrade: 1.4.x refuses a mix with virtual output sources (`virtual_outputs`): it moves `mix.json` to `mix.json.bad` and starts empty. Before going back, silence every virtual output (`DELETE /api/v1/virtual-outputs/<n>`); their routing is lost, and the 1.4 mixer has no virtual outputs (the designer's wiring to them goes dead).
- D1: a fader position block has a threshold (`threshold_db`, −120 to +15 dB, default 0 dB; the "High at" column left of Value): where it feeds an on/off input (also through macro ports) it is on from that position (before: from 0.5 dB); without a signal it is off.
- Upgrade: a fader feeding an on/off input is now on from 0 dB (was +0.5 dB); check live graphs with faders at 0 dB.
- D2: "Copy logic to" copies all blocks and cables of a graph into a new graph (dry run) or into an existing graph (below its blocks, one undo step there). Copy and paste rename only the block ids that clash (before: every pasted block got a new id).
- D3: HTTP webhook and TSL tally out drop onto the canvas like every block (they asked first, away from the drop, and were never made); a webhook's URL may be empty (inert, amber) until typed in the list, a TSL out starts without an index (`null`, inert, amber). Two `tsl-out` blocks of the show on one index are marked with a warning (not refused).
- D4: logic action "Route input → output" (`set-route`, `input`, `output`): while its input is on, a virtual output takes the input straight through; off clears it if it still has that input. Two in one graph on one output are refused, in two graphs marked.
- Fixed: a memory leak of about 16 kB/s per Silero VAD (test-all: OOM at the 1 GiB limit about every 2.75 h, since 1.3.0). `ort` 2.0.0-rc.14 leaks an `OrtTypeInfo` (about 290 bytes) for every output tensor it creates itself, two per inference (31 per second per detector). The two outputs are now preallocated once and reused. `tests/silero_memory.rs` checks that 3750 inferences keep no heap (2.2 MB before).
- A1: the browser tab's title names the tab and the node ("Console – <label> – MXL Audio Mixer").
- A2: strips of 7.1 and wider are wider (124 px) and their meter bars narrower, so the fader stays usable; the strip meter only shows whether there is signal.
- A3: a muted strip's meter still shows its level (it now reads after the fader and before the mute), grey instead of green.
- A4: the gear and ⋮ sit in the format row ("mono ⚙ ⋮"); the name has the strip's full width.
- A5: a click on the fader value types a level (0, -20, -inf), Enter applies, Escape keeps it.
- A6: the strip shows its send levels only (no send faders); a click opens Routing.
- A7: MIDI control from the browser (Web MIDI, https or localhost): Mackie Control (banks of 8, motor faders, mute, solo, select, V-pot pan, master, display; extender) and generic MIDI learn (fader, mute, PFL, AFL, pan; kept per browser, export/import JSON).
- A8: a strip with a surround panner shows where it pans (speaker ring with the source) and moves it by drag; a mono or stereo strip sending to a surround bus offers "+ Surround pan".
- A9: meters above 3 channels have narrower bars with the channel labels (L R C LFE …) below, a little less tall.
- A10: the surround panner pans into 5.1.4 and 7.1.4 too, with a height (`elevation_deg` 0–90: bed to top layer); front/rear on the strip's picture, height on the strip. A plan node holds up to 24 values (a stereo strip into 7.1.4).
- A3 behaviour change for the platform: the metric `mxl_audio_mixer_true_peak_dbtp{signal}` and `GET /api/v1/meters` (also the far side's meters over the remote link) read a muted strip's level now, not silence. Logic Level and Above blocks still read a muted strip (its mute, a muted VCA, a mute group) as silent.
- Fixed: the meter tick no longer runs back-to-back after a late tick (missed ticks are delayed); the second read found the peaks empty, so wide signals flickered to −∞ and logic level blocks saw false edges.
- Faster gain steps on the DSP thread: a gain reads only its channel's ramp values instead of copying the whole node per channel (lab plan of 40 strips and 5 buses, 96 samples: 1.4.3 22.6 µs, this branch before 25.1 µs, now 22.9 µs per block).
- Downgrade: a show is read by 1.4.3 unless a surround panner has a height (`elevation_deg` above 0, written only then) or a 5.1.4/7.1.4 layout; set those back to 0 and 5.1/7.1 before a rollback.
- Fixed: a loudness measurement kept every 100 ms block and short-term value until it was reset (about 160 B/s per meter, and integrated loudness and LRA recomputed over the whole list at every meter tick). Both are now histograms of 0.1 LU bins (count, power and loudness sum per bin) with the same gates (absolute −70 LUFS, relative −10 LU and −20 LU, LRA 10th–95th percentile): constant memory and time. Against the old lists: tone and steps identical, noise and speech-like sequences within 0.003 LU (integrated) and 0.042 LU (LRA). `tests/loudness_memory.rs` checks 2 h of segments keep no heap (2 MB before).
- B1: copy and paste of processing units: copy on a unit, on any chain "Paste all", one unit, or onto a unit of the same type (its parameters).
- B2: multiband and dynamic EQ show the gain reduction of every band at all times (`band_reduction_db` in the node meters).
- B3: new `leveler` node for buses and streams (inspired by the idea of master_me, own code): slow gain riding to a short-term loudness target, gate, max gain/cut, speed, optional 1 ms safety limiter.
- B4: flow view below the processing chain: reorder nodes, move sends, the direct/bus out and keys to another tap by drag; outputs take a `tap`.
- B5: surround panner spread drawn as a translucent bubble around the source that grows and drifts to the centre, at 100 % over every speaker; the dot stays at the panned position (its distance from the centre is the height, A10).
- B6: per-send surround panner (`pan` on a send) for a mono or stereo source into a 5.1 or bigger bus, edited in the Routing tab's send editor.
- **Downgrade:** a show that uses a send's `pan`, an output's `tap` or a `leveler` node cannot be loaded by 1.4.x (unknown fields there): a rollback to 1.4.3 keeps the file as `mix.json.bad` and starts with an empty mix unless those are removed first.

## 1.4.3 (2026-10-24)

- A flow that goes away and comes back on the same route (a writer restart) keeps the last lag: its new reader is counted by the stall rule (0.5 s, or back within a block), not the 0.1 s start span. Only a route to another flow forgets it (D243). Before, a re-created flow whose writer started behind could raise the delay for about a minute.

## 1.4.2 (2026-10-23)

Smoother start-up of the inputs (test-all, 1.4.1 at `MIXER_READ_OFFSET=2`: losses only in the first 5 min while the delays settled, 236 samples on each local gateway input and 5793 on a mirrored one, whose delay peaked at 74.9 ms; PR #47; IMPLEMENTATION_PLAN D241–D243). Release notes: `docs/release-notes/1.4.2.md`.

- An input starts at the automatic delay its flow had in the last run (`MIXER_STATE_DIR/input-delays.json`, per flow id, written when the delay changes): a floor for one hold time, and an outlier of that size recurs at once, so a gateway's rare outliers are covered from the first block.
- The start-up phase: in an input's first minute, until a second outlier comes, a late block's short floor and the period of lower need last 5 s instead of the hold time. A backlog at the start (a mirror still filling, a first outlier) raises the delay at once and lets it fall within about 10 s instead of holding it for a minute. A recurring outlier still gets its recurring floor.
- A new reader's lags count after 0.1 s of a head that keeps real time (0.5 s after a stall): 1.4.1 lost the first 0.5 s of a batched writer (a browser source, gateway bursts) before its first delay.
- Lab (fresh start / restart with the saved state, 5 min, missing samples per input: gateway +6 ms / 30 s, gateway +5 ms / 40 s, mirror with a 75 ms backlog, gateway bursts, browser source): 1.4.1 576/192/0/10368/23232 and 192/192/0/11712/22848; 1.4.2 288/288/0/2208/6240 and 0/0/0/96/0. The mirror's backlog delay (47–81 ms) falls in 15–20 s (1.4.1: 60 s).
- `mxl-mixer-tone --start-behind-ms X --catch-up R`; `tests/integration/inputs.sh` checks a mirror with a backlog; its outlier checks allow the falls of the start-up phase (the rare outlier input counts from its third outlier, the F2 input may fall once in its first 28 s; the steady state stays covered by unit tests).

## 1.4.1 (2026-10-22)

From the 1.4.0 deployment on test-all (PR #44; IMPLEMENTATION_PLAN D235–D240, X34). Release notes: `docs/release-notes/1.4.1.md`.

- Fixed: the ST 2110 gateway inputs lost a whole block every 30–50 s at `MIXER_READ_OFFSET=2` (1112–1718 samples in 10 min each). Their writers are on time except for a rare packet several ms late, far below the 99.9th percentile, so the automatic delay never covered it and the hold kept it low. A late block (its arrival lag above the read offset plus the delay, the head moving) now raises that input's delay at once to its lag plus 1 ms (within `MIXER_INPUT_DELAY_MAX_MS`), also before the first second of lags and regardless of the hold. Its need stays a floor for one hold time (a one-off late block costs that latency for about two hold times, 1 min); when an outlier of about the same size (within 1 ms and a block) comes again within ten hold times (5 min) it recurs, and its floor then lasts ten hold times, renewed by each recurrence of that size, so a recurring outlier costs one or two blocks instead of one per occurrence. Other sizes renew nothing: one higher spike or smaller outliers do not ratchet the delay up. Lags beyond the read offset plus `MIXER_INPUT_DELAY_MAX_MS` (a writer catching up) count for nothing. The DSP thread only reports lags; the control tick decides.
- Fixed: deadline misses after a start (test-all: 497, and 313 with the device IRQs moved off the DSP core, in the first 3 min). A read waits up to a quarter block for samples that are almost there, and every late input waited its own quarter block: four or more late inputs took longer than the block, so every block missed its deadline. That happened at the start, before the inputs' delays were set (1.4.0 sets the first after a full second of lags; 180 misses in the lab's first second), and while several inputs were stalled (a stopped writer, a source not started yet; also in 1.3.0: 752 misses in 3 s with five). The wait is now one quarter block per block, shared by all inputs; after it the inputs read without waiting. Lab: 0 misses after a restart with 7 inputs (1.4.0: 180, 1.3.0: 31), 1 with five stopped writers (752).
- Fixed (also in 1.4.0): after a writer stopped and resumed, or a re-open, the catch-up (seconds behind, then faster than real time) was counted as arrival lag: the automatic delay went to `MIXER_INPUT_DELAY_MAX_MS` (150 ms) and stayed there for the lag window. The lag now counts only once the writer has caught up (back at its lag from before the stall, or a head that kept real time for 0.5 s); a writer always later than the window is still shown in the lag metrics, and lags beyond the read offset plus `MIXER_INPUT_DELAY_MAX_MS` move no delay (also not through the 99.9th percentile).
- Fixed: an input re-opened on a flow that was deleted and re-created within about one control tick (a writer restarted at once), or read by two inputs, could get the old reader back from libmxl's per-instance reader cache, mapped on the deleted files: the input stayed stalled. Inputs now share an MXL instance only per flow generation (flow id and the inode of its data file): a re-created flow gets a new instance, inputs on the same flow share one (each instance is an inotify instance and a watcher thread, limited per uid). It goes with its last reader, which the control side drops. A waiting input no longer creates an instance per tick, and a failed instance names the inotify limit.
- `tests/integration/inputs.sh`: a per-packet writer with one packet 6 ms late every 8 s loses nothing after its first outlier; five stopped writers cause no deadline misses, and their delays stay where they were after they resume.

## 1.4.0 (2026-10-22)

Operator feedback (IDs as in IMPLEMENTATION_PLAN "1.4.0") and two input fixes from the test-all deployment (PRs #37–#41). Release notes: `docs/release-notes/1.4.0.md`.

### Console

IMPLEMENTATION_PLAN D186–D193 (#40, #41).

- A click on a strip's name renames it in place (Enter saves, Escape cancels); a gear next to it opens its processing (C1).
- A divider between the strips and the buses and VCAs sets how wide each side is (drag, arrow keys; double-click or Home: automatic, remembered in the browser); each side scrolls on its own (C2).
- Stereo and wider strips have a "Link" box in their input patch: choose the first channel and the others follow in order on the same input; untick it to choose each channel (C3).
- A new strip or bus is selected at once and shown (C4); "+ Strip" and "+ Bus" add several at once, with numbered names (and ids, when one is given) (C6).
- Wider strips have one polarity button: it opens a toggle per channel and shows how many are inverted (C5).
- Auto-mixes have their own dialog ("Auto-mix" in the toolbar); every strip is a tile of the same size, so a new member moves nothing (C7). The "Auto" panel keeps auto-levels, ducks and voice detection.
- The strip menu (⋮): "Send to…" (a bus, a new send at 0 dB from the strip's output), on a bus "Add source…", and "Delete…" with a confirmation that lists its sends, output, VCAs, mute groups, layouts and logic blocks (C9, C11).
- The strip menu (⋮) has "Format…" on strips and buses: the format change of the Processing view (C10), confirmed by a second press.
- Deleting a strip or bus keeps the logic blocks that name it (also a `set-talk` to one of its talkback destinations), wired, with an empty target: they no longer act on it; it removed them before. A `set-monitor` that chose it gets no source. A monitor, control-room source or talkback return that listened to it, and another signal's key on it (a compressor keys itself again, a ducker hears nothing), are cleared instead of refusing the delete (C9).
- Subgroup buses (`subgroup: true`, "Subgroup…" in a bus's menu, with a confirmation): every send into one is taken from its source's output (a strip's post-pan, a bus's post-fader) at 0 dB; its level is locked (a PUT of another level or tap is 400) and the strip shows it with a lock; the bus fader is the group level. Every change (also a snapshot recall, an import or the remote link) stores those values, so the API and the far-side desk show what is heard; clearing the flag keeps them. The `group` operation makes subgroups (C12).
- **Downgrade:** a show with a subgroup (also one made by the `group` operation) cannot be loaded by 1.3.0 (`subgroup` is an unknown field there): a rollback to 1.3.0 starts with an empty mix unless the flag is cleared first.
- A logic `vad` block without a target no longer asks for voice detection of `""`.
- A strip with an active `surround-pan` node shows its pan or balance as "surround" instead of a knob that has no effect.

### DSP and formats

From the operators' feedback (IMPLEMENTATION_PLAN "1.4.0: DSP and formats", D200–D210, X25; #38).

- C10, P3: the format of a strip or bus can be changed in operation (`PATCH` `format`; the Processing view's format selector, confirmed by a second press). The chain stays: nodes adapt to the new channel count, and those that cannot run on it (M/S / width off stereo, a surround panner off a mono or stereo strip) are bypassed, listed in the answer's `warnings` and flagged (`chain_warnings`). Sends stay and use the width mapping; a strip's patch is trimmed or extended; a published bus keeps its flow's layout. The change fades.
- D1/D2: adjustable down-mix (ITU-R BS.775 style: centre, surround and LFE levels, normalisation) in the mix settings (`downmix`; Settings), with an override on a bus (everything arriving there) and on a monitor. The default is 1.3's fixed down-mix. A change fades (not ramps) only the sends whose coefficients change; the UI applies it, and returns a bus or monitor to the mix's, after a second press. The mix's down-mix is left out of `mix.json` while it is the default (rollback to 1.3).
- D3: new node `width` (M/S / width): M/S decode and encode, width 0–200 %, ramped per sample.
- D4: EQ bands `high-pass` and `low-pass` with `slope_db_per_oct` 12/24/36/48 (as the filter node), with their curves and a slope control in the EQ editor.
- D5: new node `surround-pan` for mono and stereo strips: 5.1 or 7.1, position, spread, divergence, LFE send; it replaces the strip's stereo panner, so post-pan sends feed surround buses. A position pad in the editor.
- A ramping gain matrix (panners, the width node) costs about a third of before.

### Views (operator feedback; IMPLEMENTATION_PLAN D215–D228; #39)

- Control room: Cut is called Mute (the API field stays `cut`) (R1). Mute and solo of single monitor channels, e.g. only C or the LFE: `mute_channels`, `solo_channels` on a monitor, M and S per channel on its card (R2). Every monitor card has Remove in its header (R3). Oscillator presets are named waveform first, e.g. "Sine 1 kHz −18 dBFS (EBU R 68)" (R4).
- Processing: the nodes are compact tiles of one size in a grid (side by side and below), with smaller curves and one band's controls at a time (P1, P5). ◀ ▶ and the `[` `]` keys step to the previous or next strip or bus (P2). A double-click on an EQ or dynamic EQ handle resets that band (P4). The palette stays in view (P6). Dragging uses pointer events, so the wheel scrolls meanwhile, and the page scrolls by itself near the window's edges (P7). A click anywhere on a tile's header opens or closes it (P8).
- Routing: the matrix names its axes (sources in rows, destination buses in columns) (T1). A new send needs a second click on the same cell within 3 s; the first marks it yellow, a click elsewhere cancels (T2). A strip can be published as its own MXL flow and NMOS sender: its direct out, post-fader before the pan (it was refused: "output signal … is not a bus") (T3). A copy button next to a published flow id (T4).
- My headphones (personal monitors) can follow the control room's PFL/AFL: `follows_solo` on `/api/v1/personal`, "Follow PFL/AFL" on the card; their own solo wins (R5).
- Logic: a click on a block's row selects it in the graph (L1).
- Graph and routing: a send's tap list offers only the taps its source has, from the new `GET /api/v1/taps` (G1).

### Input reading (F1, F2)

From the test-all deployment (D-263; IMPLEMENTATION_PLAN D230–D234, X33; #37).

- Fixed: inputs whose writer commits audio in batches lost samples at the default `MIXER_READ_OFFSET` of 2 ms (ST 2110 gateway audio delivered in bursts of packets, browser-source audio committed per video frame), all counted as late, while the arrival lag showed about 0.5 ms and no automatic delay was set. The arrival lag was measured only in blocks right after a commit, when the flow is freshest; it now counts every read block, as the distance from the block's end to the head index, so a 960-sample batch shows about 20 ms, the automatic delay covers it and the input reads clean. A block is late exactly when its arrival lag exceeds the read offset plus the extra delay. A head that stands still longer than the read offset plus `MIXER_INPUT_DELAY_MAX_MS` is a stall and adds no lag. After a new reader the lag counts from the first head move, so a new flow (head 0) or a re-opened stale one does not drive the delay to its maximum. The first automatic setting waits for one second of lag samples (it came after 100 ms).
- A read that MXL answers "too early" after its wait is tried once more without waiting: MXL wakes a waiting reader only once per sync batch of the writer and returns the result from before the wait.
- The automatic input delay holds: up at once when the 99.9th percentile of the lag comes within 0.5 ms of the delay; down only after the need stayed at least 1 ms lower for `MIXER_INPUT_DELAY_HOLD_S` (new, default 30 s), to the highest need of that time, and at most once per hold time. It changed every 0.1–1 s (with a fade) on fabrics-mirrored inputs.
- `mxl-mixer-tone` has `--batch N` (N blocks handed over at once) and `--late-ms X --every-s P --for-s D` (lag outliers); `tests/integration/inputs.sh` covers batched writers and outliers.

## 1.3.0 (2026-10-17)

Milestone 9 (§13: master/slave remote link).

- A slave accepts a master on `/api/v1/link` (WebSocket, pre-shared `MIXER_LINK_TOKEN`). The master connects to up to four `MIXER_LINK_PEERS`. The gallery console shows each far side as a layer (strips, inputs, auxiliaries, meters) and rides fader, mute and send there. The far side mixes its own voices into in-ears (`iem-<voice>`, the other voices plus the gallery return). `POST /api/v1/remotes/<id>/return` publishes the gallery mix-minus of a bus without the far-side contribution and adds that return to the in-ears. Audio stays on MXL. The slave holds its mix when the master disconnects; `MIXER_LINK_TAKEOVER=deny` then refuses local changes. `remote_link_up` and `remote_link_rtt_ms`.

### Logic editor

Milestone 8, part 2 (§11; IMPLEMENTATION_PLAN M8b, D158–D174, X11–X16).

- Typed ports (`boolean`, `number`, `level`, `event`, `string`) and type-checked wires with fixed conversions; every 1.2.0 graph stays valid.
- New blocks: `text`, `trigger` (`POST /api/v1/logic/<graph>/trigger/<block>`), `pfl`, `loudness`, `nmos-receiver`, `nmos-sender`, `link`; `toggle`, `counter`, `math`, `scale`, `map`, `select`, `compare` `ge`/`le`/`ne`; `set-vca`, `set-param`, `set-pfl`, `set-talk`, `set-publish`, `set-monitor`, `loudness-control`, `tsl-out`, `log`. `set-fader` and `set-vca` take `ramp_ms`.
- Macros (`logic_macros` in the show, `/api/v1/logic-macros`): sub-graphs with exposed ports (`macro-in`, `macro-out`), inlined for evaluation, with recursion, depth (4) and size (256 blocks per graph inlined) checks.
- `GET /api/v1/logic-blocks`, `PUT /api/v1/logic/<id>`, `POST /api/v1/logic-batch` and `POST /api/v1/logic-validate`, with a `revision` check (409); refused graphs answer 400 with `issues` (graph or macro, block, port, wire).
- Graphs are compiled once per change and run in topological order; their answers carry `revision`, `ports`, typed `values` and the last 20 `actions`. Dry run lists its actions (also on the events WebSocket and in the log). An action whose input has no wire never runs; `edge` outputs an event.
- New settings `TSL_OUT_TARGET` and `TSL_OUT_SCREEN` (TSL UMD 5.0 over UDP, on every change and every second; invalid values exit 78).
- The webhook block posts `{"graph", "block", "value"}` as documented (it posted only `value`).
- Fixed: a logic change that rewires the plan (publish, PFL, talk, monitor source, snapshot recall; recall was affected in 1.2.0 too) could be lost when an API change staged during its fade. Logic and the API now share one change lock (D167).
- `mix.json` is written within about 0.6 s after a parameter change (faders from the API or from logic; at most every 500 ms), at once after a plan change, a show action (snapshots, layouts, history, import) and at SIGTERM, and again at the end of the shutdown; it was written on every change, up to 100 times a second (D168).
- Safety (D169–D174): an action fed only by an unconnected macro input never runs; an action moved to another target acts on it at once (state is kept only for the same type and parameters); `edge` and timers do not fire on the first tick or on a re-wired input; NaN is no change and +∞ is the fader's maximum (NaN closed the fader); nested macros validate in linear time within an inlining budget, off the control lock, and a fader move no longer re-validates every graph; rate limits per action (20 ms, TSL 50 ms, webhooks 100 ms, plan changes 250 ms), one shared webhook client, at most 8 posts at once; TSL out switches a removed index off, splits at 2048 bytes and logs failures again after a recovery. an action block whose target is still empty (`""`) is valid but never acts and is listed in `warnings`; `logic-validate` honours revisions; a batch deleting a missing id is 400; descriptions have at most 1000 characters.
- `tests/integration/logic.sh` runs in CI.

The editor (the Logic tab; D175–D184):

- The Logic tab's canvas is the editor of §11.3: a palette built from the server's block catalog (drag a block onto the canvas, or click it); cables drawn between typed ports, coloured by type, and refused while they are drawn when the input cannot take them; zoom, pan, a minimap; multi-select (Ctrl-click, Shift-drag).
- Undo and redo of every edit (Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y, and buttons), per graph or macro, each one atomic batch. An undo that would overwrite another client's change is refused and the history of that graph is cleared. With text selected on the page, Ctrl+C copies the text unless the canvas was clicked last.
- Copy and paste of the selected blocks with the cables between them (Ctrl+C, Ctrl+V), with new ids, also into another graph or a macro.
- Macros: group the selected blocks into a macro and choose its exposed ports (those a cable crosses are always exposed); open a macro instance (double-click or Open) and back by the breadcrumb; ungroup. Editing an open macro changes every instance.
- Validation markers: a refused edit shows the server's issues in red on the blocks and ports, and the refused cables dashed, until the next edit.
- The debug view: every block shows its live value (on/off, dB, a count for events, text); the list of the last actions marks the dry-run ones. Trigger blocks have a Fire button.
- The list under the canvas edits every block parameter. A new block gets no target by itself: its strip, snapshot, monitor, mic, … stays empty (outlined in amber) until chosen, unless a strip is picked in the add row; an action without a target never acts, an input without one reads off, 0 or the floor and still drives what it is wired to. A webhook asks for its URL first. A choice made with the arrow keys applies on Enter or when the field is left; Escape puts the old one back.
- A new graph starts in dry run; the toolbar shows LIVE, DRY RUN or OFF. Undo and redo never switch a graph on, off or out of dry run.
- In a live graph, paste, group, ungroup, undo, redo and a new macro instance ask first when they (re)make actions or blocks that remember: those actions act at once, and re-made blocks start with empty memory. "Switch to dry run first" is offered; if that switch fails, nothing is changed.
- Fixed: a number being typed in the logic list was reset by the next live-value poll, or by a pushed mix; a field that has the focus or an unapplied edit keeps it, and is outlined in amber when the value changes elsewhere meanwhile (Enter keeps the typed value, Escape takes the stored one) (#35).
- Fixed: after a quick undo or redo the canvas could show, and a copy take, an older graph. The UI takes its logic edits from their answers at once, drops pushed mixes older than what it shows, and copies from a fresh read (#35).
- `@vue-flow/minimap` and `@vue-flow/controls` (MIT) for the minimap and the zoom buttons.

### Platform readiness

- Real time on Kubernetes: the image gives the binary `CAP_SYS_NICE` as a permitted file capability (`setcap cap_sys_nice+p`) and the mixer raises it into its effective set at start, so `MIXER_RT_PRIORITY` works for uid 1000 with `SYS_NICE` in `capabilities.add` (before, only the bounding set had it and `SCHED_FIFO` was refused). The start log lists the effective capabilities; a refused `SCHED_FIFO` is a warning; new gauge `dsp_realtime`. Without `SYS_NICE` the mixer still starts (`SCHED_OTHER`). The file capability puts the binary in glibc's secure mode (`LD_LIBRARY_PATH` and `LD_PRELOAD` are ignored; the image finds libmxl through `ldconfig`).
- The DSP thread takes the last CPU whose SMT siblings are all in the pod (a whole core), not just the last CPU. The README and `deploy/k8s` ask for an even CPU count (4 preferred; 2 is the minimum, not measured, and puts the non-DSP threads on the DSP CPU's sibling) and `reservedSystemCPUs`; `full-pcpus-only` is recommended, no longer required.
- Start-up: `MXL_DOMAIN_SCAN_PATH` must exist (78), `MIXER_STATE_DIR` must be writable (78, before: the changes were lost), libmxl that cannot be loaded exits 75 (was 78), and every port is checked before the output domain is created; a failed start (also one halfway through creating the domain) removes a domain it created.
- `MXL_OUTPUT_DOMAIN_DIR` must be directly below `MXL_DOMAIN_SCAN_PATH` (78); nothing else is ever removed.
- An existing `domain_def.json` with another id is kept and used, with a warning (was exit 78), and that domain is not removed on exit even with `MXL_CLEANUP_ON_EXIT=true` (a warning).
- `domain_def.json` and `options.json` are created atomically and only once. A private temp file (created exclusively, `create_new`, with a random UUID name) is hard-linked into place, which never replaces an existing file. Several processes creating the same domain at once all succeed, and exactly one of them counts as having created it. The domain filesystem must support hard links (tmpfs, ext4 and xfs do). The temp files are hidden: `.domain_def.json.<uuid>.tmp`, `.options.json.<uuid>.tmp`.
- Fixed: `mxl-mixer-tone` writers started together on a new domain could exit with "not a valid domain definition" (the `auto.sh` CI flake).
- An unreadable `domain_def.json` reports the real error (e.g. "cannot read …: Permission denied"). A half-made domain (`options.json` failed after `domain_def.json` was written) is removed by the failed start.
- The soft open-files limit is raised to the hard limit, at most 65536.
- Metrics: `logic_ticks_total`, `logic_actions_total{graph}`; `# HELP`/`# TYPE` for `remote_link_up` and `remote_link_rtt_ms`. `webrtc_clients` is dropped from the spec (`preview_readers{path}` counts them).
- A failed logic webhook is logged as a warning, without the URL's credentials, query and fragment. The README says the API has no authentication and belongs on the pod network.
- The version is 1.3.0 in `Cargo.toml` and `web/package.json` (1.1.0 and 1.2.0 still reported 1.0.0); a release tag that does not match them fails the publish workflow.
- `deploy/k8s`: the released tag 1.3.0, `MXL_OUTPUT_DOMAIN_ID`, `MIXER_RT_PRIORITY=49`, `SYS_NICE` with `drop: [ALL]`.

### Remote link

Fixes from the audit of `09b5598` (IMPLEMENTATION_PLAN X5–X10, D147–D157).

- `wss://` works: the master speaks TLS (rustls, the system certificate store). It failed on every attempt before. The slave needs a TLS-terminating proxy in front of its web port. Over `ws://` the token travels in clear: keep such a link on a trusted network.
- Takeover deny no longer refuses the master's own commands, and a closing extra connection no longer marks the link down under a live master.
- A dead link is noticed: both sides ping every second and close the link after 5 s of silence (or a send stuck for 5 s); the master reconnects. The hourly reconnect is gone.
- The slave takes one master at a time; a second one is refused with "another master is connected".
- The RTT is measured on one host's clock (the side's own ping and pong). On the slave, `remote_link_up{peer="master"}` goes to 0 when the master goes.
- A message type the peer does not know is ignored (logged once per connection) instead of closing the link; an unknown command is refused with an `ack`.
- Commands and pushes are numbered per connection and acknowledged; delivery is at most once: what was not sent when the link drops is dropped, and `command`, `push` and `return` answer 409 while the far side is down.
- A hello must come within 5 s, and a link message may have 4 MiB at most (was 64 MiB).
- `MIXER_LINK_PEERS` must be `ws://` or `wss://` URLs with a host and a valid port, each slave once (exit 78). A slave's id is its place in the list (`s1` first), not the order in which slaves answered.
- `return` is refused while `ret-<peer>` is another signal; it used to leave a new bus and output behind on every rebuild.
- A fader at −∞ no longer breaks the link: levels carry `"-inf"` (the `null` of older peers is read as −∞). Before, one such fader made the master ignore every later state of that far side.
- `command` checks the link and queues under one lock, so nothing queued after the link dropped goes out on the next connection; at most 1024 commands wait per far side (503). The 409s and the 503 have a JSON `error`.
- The master lists its slaves from the start (`remote_link_up` 0 for one never reached), and the RTT is left out while a link is down.
- The token is compared as a digest (its length does not show in the time taken), computed once at start; a wrong one is logged on the slave at most every 10 s. A user or password in `MIXER_LINK_PEERS` is refused. An unreadable push gets a refusing `ack` too. The link's timers do not burst after a stall.
- README: `MIXER_LINK_TOKEN` is a secret (the README said no setting was); `SSL_CERT_FILE`/`SSL_CERT_DIR` for a private CA.
- CI runs `tests/integration/link.sh`, which now also checks a hung master and far side, takeover deny, a second master, a silent and an oversized connection, and `wss://` through a TLS proxy with a private CA.

### Tests

- The §16 tone writers (`tests/perf/section16.sh`) run off the DSP core at `SCHED_FIFO` `MIXER_RT_PRIORITY` − 4 (at least 1) when allowed; the script takes `MIXER_RT_PRIORITY` only as an integer 1–49 (`off`, `false` or 0 leave them `SCHED_OTHER`).
- The m0 spike keeps its other threads off the block core; `m0-block-clock` picks the CPU by the whole-core rule. The lint job also runs the m0 spike's tests.
- The test scripts' `urllib` calls use an opener without proxies (`no_proxy` CIDR entries were ignored).
- Flakes fixed: `auto.sh` ducking (a fixed input delay, and the dump taken again after a late input or a DSP deadline miss, at most 4 times; #30, #31), `processing.sh` (polling instead of a fixed wait; #30) and `ui.mjs` (the auto-mix shares taken once they have settled for 1 s; #34).

### Documentation

- Release notes for 1.2.0 (`docs/release-notes/1.2.0.md`: the image digest, the hardware run) and 1.3.0 (`docs/release-notes/1.3.0.md`: every setting, ports, volumes, widgets, metric families, what the platform must set, upgrade notes, the catalog facts).
- The 1.3.0 notes say how browser talkback's WHIP path meets the shared MediaMTX's publish rule, and that the mixer sends no WHIP credentials.
- `docs/release-notes/1.0.0.md` corrected: `/readyz` does not wait for the TAI offset (watch `timing_trusted`).
- README: the status line; port 3343 is reserved for IS-07 events and not opened. The specification lists the settings it did not name (§14.1). The plan's status table is current.

## 1.2.0 (2026-10-15)

Milestone 8 (§19: the logic editor). Release notes:
`docs/release-notes/1.2.0.md`.

### Milestone 8 (part 1): logic graphs

- Logic graphs in the show (§11): blocks (`const`, `button`, a signal's mute,
  fader, voice and level, tally on air, a timer, a meter with hysteresis,
  snapshot recall, `and`, `or`, `not`, `compare`,
  `edge`, `latch`, `set-mute`, `set-fader`) wired port to port. A loop is allowed
  only through an `edge`, a `latch` or a `timer`. A timer stays on for its
  duration after its input rises (rounded up to the 10 ms tick); a new rise
  starts it again. Above turns on when a signal's peak reaches a threshold and
  stays on until the peak falls by the hysteresis. Recall runs a snapshot when
  its input rises (not when the graph first loads). XOR, a delayed copy, a
  debounce, a time-of-day schedule and a webhook (POST when the input rises)
  are blocks too. Evaluated at 100 Hz; actions run when their input
  changes; `dry_run` computes without acting. `/api/v1/logic`. The Logic tab
  draws the cables and lists the same graph under them.

## 1.1.0 (2026-10-14)

Milestone 7 (§19: VAD, auto-level, auto-mix, ducking). Release notes:
`docs/release-notes/1.1.0.md`.

### Breaking default: ports

- `WEB_PORT` defaults to **8190** and `NMOS_PORT` to **3342** (the NMOS events
  WebSocket is 3343). 1.0.0 used 8160 and 3312, which another function on the
  same nodes already uses. Set `WEB_PORT=8160` and `NMOS_PORT=3312` to keep the
  old ports. The image `EXPOSE`, the Kubernetes example, the demo and the
  AMWA defaults follow.

### Missing input samples, split by cause

- `input_overrun_samples_total`: samples the ring had already overwritten
  (the reader fell behind). `input_late_samples_total` stays the source that
  had not written the samples yet. Other errors stay in
  `input_missing_samples_total` only. The §16 load line prints all three, and
  the inputs table shows overrun next to late.
- The ONNX Runtime download uses the system certificate store. `ORT_LIB_LOCATION`
  is the extracted library directory when the download is done by hand (the
  archive is raw LZMA2).

### Milestone 7 (part 1): voice detection, auto-mix, ducking

- Voice activity detection (§8.1) on the control side: per analysed signal
  (before its fader) speech active, probability, speech level, onsets and
  offsets; band energy over the noise floor, voicing, modulation and tone
  rejection on 10 ms frames, threshold with hysteresis and hold. `/api/v1/vad`,
  `vad_active{signal}`.
- Gain-sharing auto-mixer (§8.3, after Dugan): NOM attenuation, weights, bypass,
  last-mic-hold, VAD weighting; muted members leave the sharing.
  `/api/v1/automixes`.
- Ducking by speech (§8.4): targets down by a depth while a key speaks, with
  attack, hold and release; also speech priority. `/api/v1/ducks`.
- An auto gain after the fader of auto-mix members and duck targets; the VAD
  state and the auto gains in the meters message; the console's Auto panel, a
  voice dot and the auto gain on strips.
- `mxl-mixer-tone --wav` (`--gap-s`, `--offset-s`): a recording instead of the
  tones.
- After the maintainers' answers and Com-IS-Assist's measurements: the auto gain
  is a chain node (`auto`), before the fader by default; snapshots hold the auto
  setup (section and class `auto`, recall-safe auto-mixes and ducks); the VAD
  and duck holds are 800 ms; a gate on the momentary loudness (`gate_lufs`,
  −32 LUFS) against crosstalk replaces `min_level_dbfs`.
- `docs/release-notes/1.1.0-checklist.md`: tests on recordings of productions.
- Silero VAD (ONNX Runtime, the model vendored, MIT) is the default detector;
  the feature detector stays, per signal (`detector`) and as the fallback.

### Milestone 7 (part 2): auto-level and ratio ducks

- Com-IS-Assist's control core (Andreas Weiss, MIT) copied into
  `src/auto/comis/` with its notice: the four-state machine, the slow, mid and
  fast stages, the automix engine, their tests.
- The VAD taps carry every channel (records in one ring, `vad_drops_total`);
  BS.1770 loudness per tapped signal on the control side (momentary,
  short-term, a fast 50 ms reading, the channel weights); the VAD's gate uses
  it, and its state shows `short_term_lufs`.
- Auto-level (§8.2): `/api/v1/autolevels/<signal>`: a target (−23 LUFS
  short-term), a speech offset (+2 LU while the VAD speaks), a range (±12 dB),
  attack and release (1 s, 3 s), a freeze below −50 LUFS.
- Ratio ducks (`mode: "ratio"`): Com-IS-Assist's control, the keys held 5 LU
  above the targets (13 LU over-voice), interview passthrough when only the
  targets speak, at most 24 dB, a speed. Channel depth groups for both modes
  (front, centre, LFE, surround) and per-channel auto gains.
- Auto-levels in snapshots (the `auto` section, recall-safe); the meters
  message carries the auto-levels' and ratio ducks' states; the Auto panel
  edits both; the demo ducks its music to a ratio and auto-levels its voices.

## 1.0.0 (2026-10-13)

The first release: milestones 0–6 (§19). Release notes:
`docs/release-notes/1.0.0.md`.

### After the hardware re-run

- The mixer keeps its other threads off the DSP thread's core (its CPU and that
  CPU's SMT siblings) and says in `dsp.scheduling` whether the core is shared
  (a warning when a sibling is outside its CPUs); the platform must give the
  DSP thread a whole physical core (`full-pcpus-only`, or SMT off).
- The meter thread no longer allocates per block.
- `ci/amwa.sh`: the node's ports from `AMWA_NODE_PORT` and `AMWA_WEB_PORT`;
  the load test prints the DSP thread's core.

### After the first hardware round

- The DSP thread's load in the §16 test: 0.854 on the maintainers' Xeon Gold
  6136, 0.149 now on the development VM (0.378 before):
  - the meters (peak, true peak, PPM, energy, correlation) on a meter thread of
    their own, from blocks the DSP thread hands over (`meter_drops_total`);
  - true peak four phases at a time (3.6× faster), skipped where it cannot
    exceed the peak already held;
  - the dB conversions without libm (`log2`/`exp2` by bits and series, error
    below 1e-8 dB), the compressor and de-esser in passes over the block that
    vectorise; biquad cascades sample by sample through all sections (the same
    arithmetic); peak and energy sums that vectorise.
- `tests/hw/release/run.sh`: the `amwa` row as text (no `|` in the table), the
  node's name from `NODE_NAME` (in a container); `ci/amwa.sh`: its
  `/dev/shm/mixer-amwa.*` folders removed, the registry's ports from
  `AMWA_REGISTRY_PORT` (and a clear error when they are taken).

### Before the release

- The routing graph (§5): a Graph tab with inputs, strips, buses, monitors and
  outputs; sends drawn, edited and deleted in it (Vue Flow, MIT).
- Oscillator: white and pink noise (level as RMS, independent channels) and a
  logarithmic sweep (`sweep_to_hz`, `sweep_s`).

### Milestone 6 (part 2): N-1 and group operations

- Mix-minus buses (`minus: {of, without}`): every send into another bus except
  some, following it without a plan swap.
- Operations (§5): group (a subgroup bus taking over the strips' post-fader
  sends), N-1 per participant or of a selection, mix-minus, link (mono strips into
  a stereo or surround strip) and unlink. `/api/v1/operations/…`; the console's
  Operations panel; mix-minus in the bus settings.
- `mxl-mixer-probe --correlate`.

### Milestone 6 (part 1): VCAs and mute groups

- VCA (DCA) groups (§9): nested, the members' faders offset by every VCA above
  them, VCA mute without touching the members' mutes, PFL/AFL of all members,
  spill on the console, in snapshots (section `vcas`), recall-safe; never a plan
  swap. `/api/v1/vcas`.
- Mute groups. A mute-safe (isolated) strip or bus is muted neither by a mute
  group nor by a VCA. `/api/v1/mute-groups`.
- Faders (strips, buses, VCAs) go to +15 dB; a fader and its VCAs add up to at
  most +15 dB. VCAs in layout pages and the `fader-bank` widget. Deleting a
  signal takes it out of layout pages.
- UI: VCA strips beside the buses, the VCAs and groups panel, mute group buttons,
  VCA chips on the strips; the `vca` widget.

### Licence

- MIT, as before. On 2026-10-12 `main` was GPL-3.0-or-later for a few hours; that
  was taken back the same day, before any release.

### Milestone 5 (part 3): talkback

- Talkback (§10.2): mics on MXL input channels and browser microphones (WHIP to
  MediaMTX, read back over RTSP, Opus decoded with loss concealment, a jitter
  buffer); destinations on buses, mixing or interrupting (IFB) with per-mic
  priority; the control room's talkback dim (`talkback_dim_db`); latching or
  momentary talk; listen-back of a destination's return; strips patched to a
  talkback mic (`{"talkback": "<mic>"}`).
- `/api/v1/talkback…`, `/api/v1/listen-back`; the talkback panel in the Control
  room tab; the `talkback` widget.
- `mxl-mixer-probe --onsets` (burst onsets in the TAI second, for latency).

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
