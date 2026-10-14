# Development environment

Everything below runs without special hardware. Real-time scheduling, multi-node
mirrors and long soaks are run by the maintainers with the scripts in `tests/hw/`.

## Base

- Linux x86_64 (Ubuntu 24.04 tested; WSL2 or a dev container work the same).
- Rust 1.97.1 (`rust-toolchain.toml` in each crate; rustup installs it).
- `clang`/`libclang-dev` (the MXL bindings run bindgen at build time), `cmake`,
  `ninja-build`, `pkg-config`, `uuid-dev`, `curl`, `zip`, `unzip`.
- Docker. Without systemd: `sudo dockerd > /tmp/dockerd.log 2>&1 &`.
- Python 3.12 with `venv` and headers, for AMWA nmos-testing.

## Kernel TAI offset

All MXL timing and the IS-05 scheduled activations use `CLOCK_TAI`. Check:

```sh
python3 -c "import time; print(time.clock_gettime(time.CLOCK_TAI) - time.clock_gettime(time.CLOCK_REALTIME))"
```

`37.0` is right. `0.0` means the kernel's TAI offset is unset (common on VMs and
CI runners); set it with `sudo scripts/set-tai-offset.py` (lost on reboot) or run
chrony with `leapsectz right/UTC`.

## libmxl at the platform pin

```sh
scripts/build-mxl.sh            # dmf-mxl/mxl 218ddaa, -DMXL_ENABLE_FABRICS_OFI=OFF, into /opt/mxl
```

About 10 minutes the first time (vcpkg builds stduuid, spdlog, fmt, picojson). The
Rust code uses the official bindings from the same commit in `mxl-not-built` mode
and loads `/opt/mxl/lib/libmxl.so` at run time (`MXL_LIBRARY` overrides).

An MXL domain root for local tests: any tmpfs directory, e.g.
`mkdir -p /dev/shm/mxl-root` and `MXL_DOMAIN_SCAN_PATH=/dev/shm/mxl-root`.

## NMOS: registry and AMWA nmos-testing

The nmos-cpp registry and the AMWA tool are set up as in `LeeO86/mxl-nmos`
(`docs/development.md` there): the registry image from `mirror.gcr.io` (no Docker
Hub limits), nmos-testing from source at commit
`90018513758e6fbc096c26d7ef001f700b28b510` in a venv. The suites run against a node
with `ci/amwa.sh` of `mxl-nmos`:

```sh
MXL_DOMAIN_SCAN_PATH=/dev/shm/mxl-root ../mxl-nmos/ci/amwa.sh off spikes/m0/target/release/m0-spike
```

## Milestone 0 spike

```sh
cd spikes/m0 && cargo build --release
NMOS_HOST_ADDRESS=<your IPv4> MXL_DOMAIN_SCAN_PATH=/dev/shm/mxl-root SPIKE_DURATION_S=120 \
    target/release/m0-spike            # prints a JSON report at the end
```

Connect its receiver to a flow with an IS-05 PATCH on
`/x-nmos/connection/v1.2/single/receivers/<id>/staged` (see
`tests/hw/m0-block-clock/run.sh` for the loopback to its own sender).

## EBU loudness test set

`tests/ebu_loudness.rs` checks Tech 3341 and Tech 3342 on the EBU test set v5
(not committed; tech.ebu.ch refuses scripted downloads, so CI uses a GitHub copy
hosted with the EBU's permission and checks the official hash):

```sh
mkdir -p ~/ebu-loudness-test-set && cd ~/ebu-loudness-test-set
curl -fsSL -o ebu.zip https://github.com/maxwalley/EBULoudnessTestSetV5/releases/download/EBU_Loudness_Test_Set_v5/ebu-loudness-test-setv05.zip
echo "9cc500b4df83f7c21855c74dce795ef5209a752bf884253ae57d0ce512efb062  ebu.zip" | sha256sum -c
unzip -q ebu.zip && rm ebu.zip
cd - && EBU_TEST_SET=~/ebu-loudness-test-set cargo test --test ebu_loudness
```

## Web UI

Node 22 and npm. `web/` is a Vue 3 + Vite project that builds one self-contained
`web/dist/index.html`; `build.rs` embeds it (without it, the binary serves a short
note), so build the UI before `cargo build`:

```sh
cd web && npm ci && npm run build && cd -
cargo build --release --bins
scripts/demo.sh                 # tone flow, mixer on :8190 and a demo show
```

While working on the UI, `cd web && npx vite` serves it with hot reload on
http://127.0.0.1:5175/ and proxies `/api` (WebSocket included) to the mixer on 8190.

`tests/integration/ui.sh` runs `web/tests/ui.mjs` in headless Chrome
(`playwright-core`, no browser download; `CHROME`, default `/usr/bin/google-chrome`)
against a mixer on MXL, with two clients; screenshots of every tab go to
`$IT_OUT/ui/`. `tests/integration/show.sh` uses Node's built-in WebSocket client
(`tests/integration/ws_events.mjs`).

`tests/integration/widgets.sh` serves a framing page (`tests/integration/frame.html`)
from two local origins with `python3 -m http.server` (ports `WIDGET_TEST_PORT`,
8198, and the next) and checks in headless Chrome that the allowed one gets the
widget's messages and the other is refused. `tests/integration/tally.sh` sends TSL
with `tests/integration/tsl_send.py` to ports `TSL_TEST_UDP`/`TSL_TEST_TCP` (8920,
8921); `ui.sh` uses 8930/8931.

## Voice detection: speech for the tests

`tests/integration/auto.sh` (and the demo's talking host and guest) synthesise
speech with espeak-ng (`sudo apt-get install espeak-ng`; CI installs it) and play
it with `mxl-mixer-tone --wav`. To see how the VAD treats a recording (16-bit PCM
or 32-bit float WAV, any rate), with its features on the test signals and the
detection latency of synthetic words:

```sh
VAD_WAV=talk.wav cargo test --release --lib evaluate -- --ignored --nocapture
```

It prints one character per 10 ms (`#` speech). Real recordings were checked this
way (a LibriVox reading, public domain); they are not in the repository.

## Load test

`tests/perf/section16.sh` (release build) runs the §16-shaped load for 60 s
(`SECONDS`) on ports 8180/3332 and prints the mean block time, the load ratio, the
slowest histogram bucket and deadline misses.

## MediaMTX and libopus

The preview (milestone 5) needs `cmake` for the bundled libopus (`opusic-sys`
builds it) and a MediaMTX executable for the built-in instance. The pinned
release, as in CI and the image:

```sh
curl -fsSL -o /tmp/mediamtx.tgz https://github.com/bluenviron/mediamtx/releases/download/v1.20.1/mediamtx_v1.20.1_linux_amd64.tar.gz
echo "81b143f55a5d23d4a8c028d52869c14ea4a59919900528698fcc97a747fd69c6  /tmp/mediamtx.tgz" | sha256sum -c
mkdir -p ~/mediamtx && tar xzf /tmp/mediamtx.tgz -C ~/mediamtx mediamtx
export MEDIAMTX_BIN=~/mediamtx/mediamtx
```

Without `MEDIAMTX_BIN` (and no `mediamtx` in `PATH`) the mixer runs and only logs
that it cannot start MediaMTX; `tests/integration/preview.sh` needs it, `ui.sh`
checks listening only with it. The integration tests give the built-in MediaMTX
ports 8964, 8609 and 8209 (UDP), so they do not collide with a mixer on the
defaults; `preview.sh` runs a shared MediaMTX on 8974 (RTSP), 8619 (WHEP), 8219
(ICE) and 8977 (API).

Every mixer pins its DSP thread to the last CPU it may use (and keeps its other
threads off that CPU's core): a second mixer on the same machine (a demo) shares
that core and its inputs come late, which fails the
sample-exact probes. Stop it, or start the tests with `taskset` on other CPUs.
