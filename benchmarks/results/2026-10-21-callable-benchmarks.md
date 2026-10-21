# Typed callbacks: payout pipeline

The [callback payout program](../../examples/record_pipeline/README.md) takes **2.93–2.96× Rust's time** on the two 4,096-record full-selection ThinLTO cases. Selective cases take **1.56–1.66×**. The full-selection cases remain above 2× Rust. This is a separate workload from the telemetry and memory-report comparisons.

The measured compiler/runtime source is [`848477f3d06fb1afe6d4bdd93dae4b107c90229a`](https://github.com/viarius-experiments/relar/commit/848477f3d06fb1afe6d4bdd93dae4b107c90229a). The same Python source runs under Relar and CPython. It passes and returns typed function values, retains receivers in bound methods, uses a noncapturing lambda, and composes lazy `map`/`filter` pipelines without an intermediate payout list.

## Results

All values are whole-process median milliseconds. CPython / Relar is a speedup; Relar / Rust is a time ratio. Raw samples, inclusive quartiles and empty controls are retained in the linked measurement records.

### Ordinary linking

| Workload / records | Relar ms | Rust ms | CPython ms | CPython / Relar | Relar / Rust |
|---|---:|---:|---:|---:|---:|
| `gross-full-32` | 52.372 | 13.717 | 1397.401 | 26.682× | 3.818× |
| `gross-selective-32` | 28.425 | 9.778 | 631.039 | 22.200× | 2.907× |
| `net-full-32` | 57.798 | 13.716 | 1608.835 | 27.835× | 4.214× |
| `net-selective-32` | 29.035 | 9.669 | 653.090 | 22.494× | 3.003× |
| `gross-full-4096` | 49.805 | 15.759 | 1349.897 | 27.104× | 3.160× |
| `gross-selective-4096` | 20.530 | 11.416 | 506.540 | 24.673× | 1.798× |
| `net-full-4096` | 54.897 | 15.765 | 1517.993 | 27.652× | 3.482× |
| `net-selective-4096` | 21.592 | 11.855 | 539.023 | 24.964× | 1.821× |

### ThinLTO

| Workload / records | Relar ms | Rust ms | CPython ms | CPython / Relar | Relar / Rust |
|---|---:|---:|---:|---:|---:|
| `gross-full-32` | 46.083 | 13.117 | 1421.626 | 30.850× | 3.513× |
| `gross-selective-32` | 23.809 | 9.101 | 650.319 | 27.314× | 2.616× |
| `net-full-32` | 48.566 | 13.014 | 1596.846 | 32.880× | 3.732× |
| `net-selective-32` | 24.420 | 9.120 | 658.639 | 26.971× | 2.678× |
| `gross-full-4096` | 44.009 | 14.999 | 1357.509 | 30.846× | 2.934× |
| `gross-selective-4096` | 17.484 | 11.224 | 512.799 | 29.329× | 1.558× |
| `net-full-4096` | 48.051 | 16.234 | 1552.750 | 32.315× | 2.960× |
| `net-selective-4096` | 18.145 | 10.961 | 523.207 | 28.834× | 1.655× |

## Workload and method

Each process parses one record file, repeatedly selects eligible records by a bound risk-policy callback, computes either gross amounts or net amounts after a fee, and filters out nonpositive payouts. It writes one complete count/total/largest report and a repetition checksum. Full selection uses a risk limit of 99; selective selection uses 9. Both inspect every input record; selective cases perform fewer transforms and reductions, rather than exiting the scan early.

Inputs use a fixed random seed, with 32 or 4,096 records. Each process inspects 8,388,608 input records across 262,144 or 2,048 complete queries respectively. Each condition has a correctness gate, two warmup rounds and twelve measured rounds. All six permutations of Relar, CPython and Rust occur twice among measured rounds; warmups and cross-round transitions are not balanced. CPU affinity is fixed to CPU 31 on an AMD EPYC-Milan host. These are shared-host measurements, not isolated-hardware guarantees.

Timing includes startup, parsing, all queries, checksum formatting and the final report write/close. Complete result and filesystem checks run outside the timed interval. Warm filesystem caches apply and no `fsync` is requested. Empty controls are not subtracted and omit nonempty parsing costs.

| Linking | Empty Relar ms | Empty Rust ms | Empty CPython ms | Maximum Relar control/workload |
|---|---:|---:|---:|---:|
| normal | 2.374 | 2.412 | 31.030 | 11.56% |
| thinlto | 2.430 | 2.351 | 31.480 | 13.90% |

CPython is 3.14.5 with PGO/LTO, the GIL enabled and JIT disabled. Native builds use Clang 18 and the pinned Ohm Rust 1.98.1-dev toolchain (LLVM 22.1.8), at optimization level 3. Both native engines use the named ordinary or ThinLTO mode; ThinLTO uses import/inline budgets of 1,000. Separate batches have separate interleaved baselines, so their differences do not isolate linking alone.

Rust stores records contiguously and uses native `i64` arithmetic with checked overflow and a `BigInt` retry over retained arbitrary-precision input. Its iterator closures can inline; each query and result passes through `black_box`. Relar uses typed records and first-class callback values. Closed-target analysis preserves safe borrowing and removes some dispatch/error checks, but this example still has multiple possible transform callbacks. The measurements compare these concrete implementations and storage/dispatch costs, not a universal language ranking.

## Validation and retained evidence

Each exact native pair passed all **122 independent application cases** against CPython, including huge integers, overflow, malformed inputs, Unicode separators, output preservation, overwritten inputs, hard links and invalid paths. Across both link modes that is 732 preparation executions. The measurement records contain 54 correctness gates, 108 warmup executions and 648 measured executions. Every recorded return status, stdout, stderr and complete filesystem result was rechecked against the frozen independent oracle during packaging; medians, quartiles, ratios and measured engine order were recomputed. This evidence package is an application benchmark record, not a claim that a separate full compiler validation matrix ran here.

The [audit](2026-10-21-callable-audit.json), [manifest](2026-10-21-callable-evidence-manifest.json) and [deterministic archive](2026-10-21-callable-evidence.tar.gz) retain frozen source, LLVM, build/preparation records, exact commands, all raw process results, final input/output files and verification helpers. Original paths and hashes remain intact. Native binaries, static archives, bytecode, toolchains and prior result archives are excluded; executable/tool identities remain hash-recorded. Reproduction requires rebuilding the frozen source and selecting equivalent local tool paths.

| Raw record | Uncompressed SHA256 |
|---|---|
| [normal](2026-10-21-callable-normal.json.gz) | `7146841e3b766971d369ba7e17907dcb7c31737663c4ca45e6f95e6ae167ae36` |
| [thinlto](2026-10-21-callable-thinlto.json.gz) | `489e67798df90a7eb916dea30a90e30fce9af10ccfe04a8bd8500ac36bd19666` |

The ordinary and ThinLTO batches completed on 2026-10-21. No profile counts or instrumented durations are used as timing samples in this report.
