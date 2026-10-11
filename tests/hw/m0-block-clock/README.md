# tests/hw/m0-block-clock

Milestone 0 (SPECIFICATION.md §19) on a target node: MXL `audio/float32` written
and read on the TAI block clock, with the DSP thread's real-time settings. The
development VM cannot run SCHED_FIFO, so the real-time numbers come from this run.

## What it sets up

- The M0 spike (`spikes/m0`, release build), an `mxl-nmos` node with one MXL audio
  sender and one MXL audio receiver, in a temporary MXL domain root under `/dev/shm`.
- The block thread: 96-sample blocks (2 ms) on `CLOCK_TAI` with
  `clock_nanosleep(TIMER_ABSTIME)`, SCHED_FIFO priority 49 (`MIXER_RT_PRIORITY`),
  pinned to the last CPU of the process's affinity mask (`SPIKE_CPU` overrides),
  timer slack 1 ns, FTZ/DAZ.
- An IS-05 activation that connects the receiver to the node's own sender. Every
  block, the thread writes a 2-channel test tone and reads the block 2 ms behind
  (`t − 96 samples`), checking every sample.

## Command

On the target node, as root (or with `CAP_SYS_NICE`), with libmxl 218ddaa in
`/opt/mxl` (`scripts/build-mxl.sh`) and the kernel TAI offset set:

```sh
sudo -E tests/hw/m0-block-clock/run.sh 10        # minutes; 60 for the §16 one-hour check
```

Inside a pod: Guaranteed QoS with whole CPUs (static CPU manager), the capability
`SYS_NICE`, the MXL root as for any media function.

## Expected results

The script prints the jitter, block work and arrival-lag distributions and exits 0
only when all of these hold:

| Check | Pass |
| --- | --- |
| SCHED_FIFO was granted | yes |
| Deadline misses (wake-up later than one block period) | 0 |
| Maximum wake jitter | below one block (2000 µs) |
| Write errors | 0 |
| Blocks not read (too early, too late, other) | 0 |
| Mismatched samples | 0 |

Please post `report.json` and the printed lines in the PR. For reference, the
development VM (SCHED_OTHER, shared CPUs) measured p50 19 µs, p99 190 µs,
p99.9 319 µs, max 1417 µs over 60 000 blocks, 0 misses
(`IMPLEMENTATION_PLAN.md`, M0).
