# tests/hw/release

The automatable items of the release checklist
(`docs/release-notes/<version>-checklist.md`) on a target node, in one run:

```sh
sudo -E NMOS_REGISTRY_ADDRESS=<registry> NMOS_REGISTRY_PORT=<port> \
    NMOS_TESTING_DIR=/tmp/nmos-testing NMOS_TESTING_PYTHON=/tmp/nmos-venv/bin/python \
    tests/hw/release/run.sh                      # env block-clock load registry amwa
tests/hw/release/run.sh registry                 # one item
LOAD_MINUTES=10 BLOCK_MINUTES=5 tests/hw/release/run.sh load block-clock
```

| Item | What | Pass |
| --- | --- | --- |
| `env` | host, kernel, CPU, the cgroup CPU limit, the kernel TAI offset, the commit (the image `git-<sha7>`) | TAI offset 37 s |
| `block-clock` | `tests/hw/m0-block-clock` for `BLOCK_MINUTES` (10) | SCHED_FIFO granted, 0 deadline misses, jitter under one block, every sample correct |
| `load` | `tests/perf/section16.sh` for `LOAD_MINUTES` (60), `nr_throttled` of the cgroup before and after | `dsp_load_ratio` < 0.5 and 0 deadline misses (§16) |
| `registry` | the mixer with `NMOS_REGISTRY_ADDRESS`/`PORT`: node, every receiver and a published bus's sender in the Query API (`/readyz` 200); restarted: the same node id and show; SIGTERM | exit 143, deregistered |
| `amwa` | `ci/amwa.sh off` (its own nmos-cpp registry; needs docker and nmos-testing) | no failures or warnings |

It builds the release binaries from the checkout (Rust, libmxl 218ddaa in
`LD_LIBRARY_PATH`, default `/opt/mxl/lib`) and writes `results.md` and each item's
log into `$OUT` (default `./release-tests-<time>`); it exits non-zero when an item
fails. `results.md` goes into the release notes ("Tested", on hardware); the
checklist's manual items are not in it. Run it as root (or with `CAP_SYS_NICE`)
for SCHED_FIFO (`MIXER_RT_PRIORITY`, default 49); in a pod, Guaranteed QoS with
whole CPUs.
