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
| `load` | `tests/perf/section16.sh` for `LOAD_MINUTES` (60), `nr_throttled` of the cgroup before and after, the DSP thread's scheduling (its CPU and whether its core is shared) | `dsp_load_ratio` < 0.5 and 0 deadline misses (§16) |
| `registry` | the mixer with `NMOS_REGISTRY_ADDRESS`/`PORT`: node, every receiver and a published bus's sender in the Query API (`/readyz` 200); restarted: the same node id and show; SIGTERM | exit 143, deregistered |
| `amwa` | `ci/amwa.sh off` (its own nmos-cpp registry; needs docker and nmos-testing) | no failures or warnings |

It builds the release binaries from the checkout (Rust, libmxl 218ddaa in
`LD_LIBRARY_PATH`, default `/opt/mxl/lib`) and writes `results.md` and each item's
log into `$OUT` (default `./release-tests-<time>`); it exits non-zero when an item
fails. `results.md` goes into the release notes ("Tested", on hardware); the
checklist's manual items are not in it. Run it as root (or with `CAP_SYS_NICE`)
for SCHED_FIFO (`MIXER_RT_PRIORITY`, default 49); in a pod, Guaranteed QoS with
whole CPUs.

`block-clock` and `load` judge the node as much as the mixer: the DSP thread
needs the **whole physical core** of its CPU (the last of the process's CPUs).
With SMT, its sibling must not run other work: on Kubernetes the static CPU
manager with `full-pcpus-only`, or SMT off. The mixer keeps its own other
threads off that core when it has CPUs on other cores; the `load` row says what
the DSP thread's core was ("a core of its own", or "shared with CPU n, outside
this process": another workload may slow it). On the first hardware rounds a
busy sibling made the DSP thread's blocks 3–4 times slower and caused nearly all
the deadline misses.

`amwa` needs internet access (nmos-testing clones the AMWA specification
repositories), the registry's ten ports from `AMWA_REGISTRY_PORT` (default
8010; with a registry already on them, set another base), and the node's ports
`AMWA_NODE_PORT` (3312) and `AMWA_WEB_PORT` (8160), which it checks are free. In a container, set
`NODE_NAME` to the node's name for `env` (else the container's host name is
reported).
