# Revision evaluation and demand details measurements

Measured on Linux in this crew container, using release C-ABI calls and a scripted catalog. These are acquisition/update timings on a shared host, not GUI frame times or daily-driver CPU measurements. The operator’s macOS 270-patch capture is not in the repository, so the fixture plus generated streams are used instead.

Baseline is `93c9440`, with the same opt-in counters added in a scratch workspace. Both builds use the checked-in external dependency versions. Counters are disabled unless `andamento_core::profile::start()` is called. The stream loads the Wheelhouse sidebar fixture, adds 100, then 300, then 1,000 synthetic issues, warms up, and alternates five plain and five detailed acquisitions, each forced fresh with an ignored fact. Nodes: 121/321/1,021. Two-card demand runs use one generated issue and the fixture’s hidden issue. Same-snapshot repeat requests and rendered-snapshot toggle validation must perform no new catalog evaluation.

Each executable is capped with `prlimit --as=8589934592`; Linux `VmHWM` is read at each size. Measurements below are one sequential baseline/after pair. Scheduling noise is substantial: an earlier run had plain medians 7.05/18.17/70.15 ms before and 5.86/15.52/57.71 ms after. Treat evaluation counts as the reliable evidence of removed work; these runs do not forecast end-to-end savings.

| Entities | Plain before ms | Plain after ms | Detailed before ms | Detailed after ms | Two-card demand after ms | RSS before/after KiB |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | 6.722 | 6.260 | 10.991 | 8.391 | 6.145 | 9420/8920 |
| 300 | 19.350 | 16.880 | 31.192 | 23.331 | 16.780 | 18104/16920 |
| 1000 | 74.687 | 63.503 | 113.890 | 83.249 | 61.051 | 47028/42796 |

## Phase counts and timings

These totals cover ten update/acquisition pairs per size. Entries are `calls / total milliseconds`. Phases nest: **do not add their elapsed times**. Before, presentation includes catalog/latents/metadata work; after, revision evaluation runs before the narrower presentation phase. Before, details includes a second catalog/latent pass; after, detail generation uses the shared evaluation. ABI flattening includes eager details. Workspace-subject lookup is a subphase of apply/presentation; no workspaces are observed in this scaling stream. Patch application and retained-workspace maintenance have separate nested counters; this change does not optimize these paths. Remaining apply time includes expiry scanning, managed-content publication and invalidation.

| Entities | Phase | Before calls / ms | After calls / ms |
| ---: | --- | ---: | ---: |
| 100 | apply | 10 / 3.365 | 10 / 6.011 |
| 100 | patch-application | 10 / 0.052 | 10 / 0.058 |
| 100 | workspace-maintenance | 20 / 0.019 | 20 / 0.021 |
| 100 | workspace-subjects | 20 / 0.007 | 20 / 0.011 |
| 100 | catalog | 15 / 3.345 | 10 / 2.489 |
| 100 | latents | 15 / 26.285 | 10 / 18.276 |
| 100 | metadata-resolution | 1170 / 4.952 | 10 / 0.014 |
| 100 | presentation | 10 / 58.462 | 10 / 31.618 |
| 100 | details | 5 / 20.968 | 5 / 10.775 |
| 100 | revision-evaluation | — | 10 / 21.026 |
| 100 | abi-acquire | 10 / 88.939 | 10 / 73.372 |
| 100 | abi-flatten | 10 / 26.535 | 10 / 16.641 |
| 300 | apply | 10 / 9.336 | 10 / 17.302 |
| 300 | patch-application | 10 / 0.057 | 10 / 0.057 |
| 300 | workspace-maintenance | 20 / 0.035 | 20 / 0.035 |
| 300 | workspace-subjects | 20 / 0.023 | 20 / 0.026 |
| 300 | catalog | 15 / 8.360 | 10 / 5.656 |
| 300 | latents | 15 / 66.896 | 10 / 46.361 |
| 300 | metadata-resolution | 3170 / 12.691 | 10 / 0.028 |
| 300 | presentation | 10 / 165.026 | 10 / 94.482 |
| 300 | details | 5 / 56.468 | 5 / 32.293 |
| 300 | revision-evaluation | — | 10 / 52.672 |
| 300 | abi-acquire | 10 / 250.832 | 10 / 205.943 |
| 300 | abi-flatten | 10 / 72.991 | 10 / 49.446 |
| 1000 | apply | 10 / 39.295 | 10 / 63.429 |
| 1000 | patch-application | 10 / 0.081 | 10 / 0.069 |
| 1000 | workspace-maintenance | 20 / 0.048 | 20 / 0.039 |
| 1000 | workspace-subjects | 20 / 0.038 | 20 / 0.031 |
| 1000 | catalog | 15 / 34.703 | 10 / 23.157 |
| 1000 | latents | 15 / 221.480 | 10 / 149.065 |
| 1000 | metadata-resolution | 10170 / 41.690 | 10 / 0.036 |
| 1000 | presentation | 10 / 644.845 | 10 / 367.981 |
| 1000 | details | 5 / 196.951 | 5 / 95.217 |
| 1000 | revision-evaluation | — | 10 / 174.927 |
| 1000 | abi-acquire | 10 / 954.812 | 10 / 733.426 |
| 1000 | abi-flatten | 10 / 260.974 | 10 / 155.639 |

The demand run produces ten cards across five revisions at every size, rather than 5×116/316/1,016 eager cards. Ten repeat requests per snapshot append no cards and cause no additional catalog or detail evaluations. The bounded core cache stores 64 cards or misses per revision; snapshots own their requested output until release, so pointer and action lifetimes do not depend on cache eviction.

## Rendered-snapshot click batches

Five toggles per size dispatch from a held rendered snapshot, then acquire current output. Validation performs zero catalog evaluations in both revisions; each changed output performs one. These timings include dispatch and new output, but exclude acquiring the snapshot used for validation.

| Entities | Before median ms | After median ms |
| ---: | ---: | ---: |
| 100 | 7.145 | 6.386 |
| 300 | 20.209 | 17.963 |
| 1000 | 80.314 | 67.257 |

Load phases (fixture plus generated facts), with patch and maintenance separated:

| Entities | Phase | Before calls / ms | After calls / ms |
| ---: | --- | ---: | ---: |
| 100 | apply | 116 / 10.708 | 116 / 10.826 |
| 100 | patch-application | 116 / 0.385 | 116 / 0.388 |
| 100 | workspace-maintenance | 232 / 0.034 | 232 / 0.032 |
| 300 | apply | 316 / 58.686 | 316 / 61.711 |
| 300 | patch-application | 316 / 0.869 | 316 / 0.915 |
| 300 | workspace-maintenance | 632 / 0.078 | 632 / 0.080 |
| 1000 | apply | 1016 / 543.843 | 1016 / 571.382 |
| 1000 | patch-application | 1016 / 2.992 | 1016 / 3.131 |
| 1000 | workspace-maintenance | 2032 / 0.283 | 2032 / 0.303 |

## Reproduction

```sh
cargo build -p andamento-ffi --example snapshot-profile --release --target x86_64-unknown-linux-gnu
prlimit --as=8589934592 target/x86_64-unknown-linux-gnu/release/examples/snapshot-profile templates/flotilla-default.kdl ../wheelhouse/data/sidebar/fixture.jsonl
```

The command also exercises five rendered-snapshot toggle/output batches at each size. These are update-stream core/ABI measurements; fixed-snapshot GUI benchmarks and Wheelhouse idle CPU remain separate. No frontend relation cache or publication batching is included.

## Validation

Independent core/frontend tests and the real C smoke client pass. Generated lifecycle comparisons cover source precedence, unsets, TTL boundaries, configuration variables, workspace observations/removal, dispatch and completions against uncached controller output. Native tests cover hidden identities, typed fields/provenance, retained expiry, targets disappearing while held, append-safe text/action lifetimes, stale dispatch/request rejection, and non-placement region diagnostics. Existing terminal/placement, ended-subject and workspace-coverage tests pass.

Mutation checks caught redundant detail catalog evaluation (the prior behavior), missing evaluation invalidation, and missing stale-request rejection; all were reverted.
