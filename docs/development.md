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

## Added in later milestones

MediaMTX (preview contract, milestone 5) and headless Chromium and Firefox (UI and
widget checks, milestone 4) are added to this file and to CI with the milestone that
needs them.
