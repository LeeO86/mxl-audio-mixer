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
scripts/demo.sh                 # tone flow, mixer on :8160 and a demo show
```

While working on the UI, `cd web && npx vite` serves it with hot reload on
http://127.0.0.1:5175/ and proxies `/api` (WebSocket included) to the mixer on 8160.

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

## Load test

`tests/perf/section16.sh` (release build) runs the §16-shaped load for 60 s
(`SECONDS`) on ports 8180/3332 and prints the mean block time, the load ratio, the
slowest histogram bucket and deadline misses.

## Added in later milestones

MediaMTX (preview contract, milestone 5) is added to this file and to CI with the
milestone that needs it.
