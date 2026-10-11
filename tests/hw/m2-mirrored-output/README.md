# tests/hw/m2-mirrored-output

§17 "Mirrored outputs": a published mixer output replicated by mxl-fabrics-agent to
a second node at the default commit size arrives at the full sample rate, without
gaps, and sample for sample as written. The commit size default
(`MIXER_OUTPUT_COMMIT_SAMPLES`, one block) is still an open point (§20); run it at
the default and at 480.

## What it sets up

- **Node A** (`source`): `mxl-mixer-tone` writes a 2-channel tone into a test domain
  `<scan path>/m2-hw-tones`. A mixer (`NMOS_SEED=m2-hw-source`, ports 8164/3316)
  reads it on input 1 through a mono strip centred into a stereo bus `pgm`
  ("M2 HW PGM"). The bus is published as an MXL flow and an NMOS sender, registered
  with the platform registry.
- **Node B** (`check`): a second mixer (`m2-hw-check`, ports 8166/3318) activates
  its input 1 with node A's sender. The fabrics agent then mirrors the flow to node
  B. The script waits for the mirrored flow below the scan path, reads it with
  `mxl-mixer-probe` 150 ms behind TAI (`BEHIND_MS`; below the domain's 200 ms
  history), and compares every sample with tone × 0.7071 on both channels. It also
  watches mixer B's input (state, arrival lag, effective delay, missing samples).

## Command

Both nodes: the repository (release build in `target/release`, or `BIN_DIR`
pointing at the binaries, e.g. from the image), libmxl 218ddaa in the linker path,
the kernel TAI offset set, and:

```sh
export NMOS_REGISTRY_ADDRESS=<registry> NMOS_REGISTRY_PORT=<registration port>
export NMOS_HOST_ADDRESS=<this node's or pod's IP> MXL_DOMAIN_SCAN_PATH=/Volumes/mxl
```

Node A:

```sh
tests/hw/m2-mirrored-output/run.sh source          # default commit (one block, 96)
tests/hw/m2-mirrored-output/run.sh source 480      # second run: 480-sample commits
```

It prints the `check` command line for node B, for example:

```sh
tests/hw/m2-mirrored-output/run.sh check <sender id> <domain id> <flow id> 60
```

Stop node A with Ctrl-C after the check. Logs and results are in
`m2-mirrored-output-<time>/` in the current directory.

## Expected results

`check` exits 0 only when all of these hold:

| Check | Pass |
| --- | --- |
| Samples read on node B in `seconds` | ≥ seconds × 48 000 − 960 (full sample rate) |
| Failed reads (gaps, too late, too early) | 0 |
| Largest sample error against tone × 0.7071, both channels | < 1e-5 |
| Mixer B input 1: missing samples while probing | 0 |

Please post the printed lines (they include mixer B's arrival lag p50/p99/p99.9 and
effective delay) for both commit sizes in the PR. A single-node dry run, where
`check` reads the source's own flow, passed on the development VM: 480 000 of
480 000 samples in 10 s, 0 gaps, error 0, arrival lag p99.9 2.4 ms.
