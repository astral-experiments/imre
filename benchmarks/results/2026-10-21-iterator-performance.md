> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Iterator performance against Rust

The unchanged [telemetry comparator](../../examples/telemetry_comparison/README.md) now takes **1.385–1.723× Rust's time** across the four 4,096-record full-scan ThinLTO cases. All four cases meet the requested **below 2× Rust** target. These are complete-program median comparisons on one shared host.

| Full scan, 4,096 records | Imre ms [p25, p75] | Rust ms [p25, p75] | CPython / Imre | Imre / Rust | <2× |
|---|---:|---:|---:|---:|:---:|
| `report-mixed` | 28.990 [28.635, 29.216] | 20.924 [20.796, 21.240] | 27.663× | 1.385× | yes |
| `alert-quiet` | 21.533 [21.222, 21.777] | 12.496 [12.264, 12.565] | 45.685× | 1.723× | yes |
| `within-quiet` | 21.112 [20.752, 21.600] | 12.628 [12.403, 12.832] | 46.330× | 1.672× | yes |
| `latest-quiet` | 22.296 [22.084, 22.883] | 15.820 [15.501, 16.017] | 37.624× | 1.409× | yes |

The [implementation PR](https://github.com/viarius-experiments/imre/pull/59) continues native Stack 4. The measured compiler/runtime revision is [`1fb6fa42fd2ee811dc6ed07702e3632a9aa62f44`](https://github.com/viarius-experiments/imre/commit/1fb6fa42fd2ee811dc6ed07702e3632a9aa62f44); the Python example and independent Rust reference are unchanged from the [previous comparison](2026-10-20-iterator-pipelines.md).

## What changed

The compiler carries integer-range facts through supported control flow, borrows scalar inputs and immutable reads while their owners remain rooted, and reuses list lengths and backing-buffer pointers only when effect analysis proves the source cannot change during iteration. Exact sequence types and cursor bounds justify narrower getters. Exact integer owners avoid nullable-object cleanup checks; heap-integer ownership paths stay in cold helpers.

Recognized pure same-local integer absolute-value conditionals lower through a select with a checked overflow boundary. Small-integer fast paths expose their representation facts to LLVM after the overflow guard. For bounded pure integer functions used directly in comparisons, the compiler can guard the incoming tags once and keep the arithmetic unboxed through the final boolean. Interval proofs reject any intermediate that could overflow signed 64-bit arithmetic; the original arbitrary-precision call remains the fallback. Arguments are evaluated once in Python order, and the unboxed intermediate never escapes as a Python integer.

These transformations preserve the checked fallback: arbitrary-precision integers, overflow behavior, left-to-right evaluation, exceptions, mutation through aliases, ownership, lazy iterator pulls, strict-zip exhaustion, and early exits. Unknown or potentially invalidating effects keep ordinary checked/owned operations. The workload has no benchmark-specific compiler entry point.

The useful design ideas from scriptc are [borrowed arguments](https://github.com/vercel-labs/scriptc/blob/139542bcf119e546305682b20ab76ebc0283ea5a/packages/compiler/src/backend/llvm/borrowed-inputs.ts), explicit [reference effects](https://github.com/vercel-labs/scriptc/blob/139542bcf119e546305682b20ab76ebc0283ea5a/packages/compiler/src/backend/llvm/reference-effects.ts), and [guarded dense array access](https://github.com/vercel-labs/scriptc/blob/139542bcf119e546305682b20ab76ebc0283ea5a/packages/compiler/src/backend/llvm/dense-array-access.ts). Imre adapts those concepts to its own Python semantics and Rust runtime; no upstream implementation was copied.

We also inspected Bun's experimental [native-AOT draft PR 45631](https://github.com/oven-sh/bun/pull/45631), specifically its pinned [typed boundary design](https://github.com/oven-sh/bun/blob/2217cacf7da4f9f5a6a9a7ed4df24ab7a5faf19c/docs/bundler/aot-types.mdx) and JavaScriptCore [B3 loop versioning](https://github.com/oven-sh/WebKit/blob/50dd4bf7cc55587ef878676a445c8bcd972ce2ff/Source/JavaScriptCore/b3/B3VersionLoops.cpp). That prototype targets JSC/B3. Released Bun [`--compile`](https://bun.com/docs/bundler/executables) packages code with the Bun runtime, and `--bytecode` moves parsing work to build time; neither is evidence that this Python workload was benchmarked against Bun or scriptc.

## Workload and timing protocol

Each process parses aligned timestamp/value files, validates complete alignment with strict zip once, then repeats queries. `report-mixed` performs signed, absolute and squared-error reductions, two threshold predicates, and a reverse search. Quiet cases inspect all records; early/recent cases stop after one pair. Full alignment is checked before those early exits. All engines render the final result once and produce a repetition checksum.

Rust uses contiguous native records, checked `i128` arithmetic, and a `BigInt` fallback. Imre uses the typed Python object/container layout and arbitrary-precision integer runtime. Both preserve the same accepted input, errors and output/filesystem effects. Rust applies `black_box` to each repeated query and result. This is a comparison of those concrete implementations, not a universal language ranking.

Canonical sizes are 16, 256 and 4,096 records per file, generated in that order. Full scans target 8,388,608 source-level inspected pairs per process; early exits target 2,097,152, rounded down to whole queries. Every condition has a complete correctness gate, eight warmup rounds and twelve measured rounds. All six engine permutations occur twice among measured rounds; warmups and cross-round transitions are not fully balanced. CPU affinity is fixed to CPU 0, timing runs outside ptrace, and build/profile processes are excluded by the recorded coactivity guards.

The host reports `AMD EPYC-Milan Processor` on `Linux-6.8.0-146-generic-x86_64-with-glibc2.39`. CPython is 3.14.5 (PGO/LTO build, GIL enabled, JIT disabled); native compilation uses `Ubuntu clang version 18.1.3 (1ubuntu1)` and `rustc 1.98.1-dev (f62703110 2026-09-08) (ohm-1.98.1-1)`. Exact binaries and target triples are hash-bound in each record.

Values below are median milliseconds. CPython / Imre is a speedup; Imre / Rust is a time ratio. Raw samples and inclusive quartiles are retained. Ordinary and ThinLTO have independent interleaved baselines, with matching native linking modes and ThinLTO import/inline budgets of 1,000; differences between those batches do not isolate linking alone.

### Ordinary linking

| Workload / records | Imre ms | Rust ms | CPython ms | CPython / Imre | Imre / Rust |
|---|---:|---:|---:|---:|---:|
| `report-mixed` / 16 | 36.410 | 26.664 | 1343.778 | 36.907× | 1.366× |
| `alert-quiet` / 16 | 41.985 | 20.772 | 1228.923 | 29.271× | 2.021× |
| `within-quiet` / 16 | 33.344 | 22.672 | 1241.034 | 37.220× | 1.471× |
| `latest-quiet` / 16 | 22.253 | 20.817 | 1000.434 | 44.957× | 1.069× |
| `report-mixed` / 256 | 23.834 | 17.887 | 816.526 | 34.258× | 1.333× |
| `alert-quiet` / 256 | 17.987 | 10.434 | 970.661 | 53.965× | 1.724× |
| `within-quiet` / 256 | 19.240 | 10.499 | 976.758 | 50.768× | 1.833× |
| `latest-quiet` / 256 | 18.516 | 12.740 | 821.960 | 44.392× | 1.453× |
| `report-mixed` / 4,096 | 30.609 | 21.043 | 796.232 | 26.013× | 1.455× |
| `alert-quiet` / 4,096 | 23.600 | 13.380 | 971.436 | 41.162× | 1.764× |
| `within-quiet` / 4,096 | 25.811 | 14.328 | 976.099 | 37.818× | 1.801× |
| `latest-quiet` / 4,096 | 25.472 | 15.897 | 821.570 | 32.254× | 1.602× |
| `alert-early` / 16 | 103.717 | 49.104 | 1364.915 | 13.160× | 2.112× |
| `within-early` / 16 | 78.629 | 48.085 | 1318.088 | 16.763× | 1.635× |
| `latest-recent` / 16 | 32.122 | 35.872 | 1134.350 | 35.314× | 0.895× |
| `alert-early` / 256 | 104.363 | 48.205 | 1377.605 | 13.200× | 2.165× |
| `within-early` / 256 | 79.490 | 50.032 | 1323.377 | 16.648× | 1.589× |
| `latest-recent` / 256 | 32.381 | 36.518 | 1137.591 | 35.132× | 0.887× |
| `alert-early` / 4,096 | 112.627 | 53.733 | 1399.903 | 12.430× | 2.096× |
| `within-early` / 4,096 | 86.666 | 54.395 | 1335.491 | 15.410× | 1.593× |
| `latest-recent` / 4,096 | 39.546 | 43.985 | 1172.376 | 29.646× | 0.899× |

### ThinLTO

| Workload / records | Imre ms | Rust ms | CPython ms | CPython / Imre | Imre / Rust |
|---|---:|---:|---:|---:|---:|
| `report-mixed` / 16 | 33.166 | 19.769 | 1316.997 | 39.710× | 1.678× |
| `alert-quiet` / 16 | 27.021 | 15.079 | 1235.452 | 45.721× | 1.792× |
| `within-quiet` / 16 | 25.507 | 15.612 | 1237.722 | 48.525× | 1.634× |
| `latest-quiet` / 16 | 17.870 | 19.249 | 1004.389 | 56.206× | 0.928× |
| `report-mixed` / 256 | 23.977 | 18.355 | 813.229 | 33.918× | 1.306× |
| `alert-quiet` / 256 | 15.766 | 10.369 | 989.290 | 62.747× | 1.520× |
| `within-quiet` / 256 | 14.684 | 10.378 | 975.910 | 66.459× | 1.415× |
| `latest-quiet` / 256 | 16.002 | 13.443 | 809.284 | 50.573× | 1.190× |
| `report-mixed` / 4,096 | 28.990 | 20.924 | 801.966 | 27.663× | 1.385× |
| `alert-quiet` / 4,096 | 21.533 | 12.496 | 983.721 | 45.685× | 1.723× |
| `within-quiet` / 4,096 | 21.112 | 12.628 | 978.116 | 46.330× | 1.672× |
| `latest-quiet` / 4,096 | 22.296 | 15.820 | 838.873 | 37.624× | 1.409× |
| `alert-early` / 16 | 61.404 | 32.051 | 1375.209 | 22.396× | 1.916× |
| `within-early` / 16 | 56.590 | 31.877 | 1315.585 | 23.248× | 1.775× |
| `latest-recent` / 16 | 16.742 | 35.241 | 1120.629 | 66.934× | 0.475× |
| `alert-early` / 256 | 61.350 | 33.933 | 1394.475 | 22.730× | 1.808× |
| `within-early` / 256 | 57.287 | 35.598 | 1331.062 | 23.235× | 1.609× |
| `latest-recent` / 256 | 17.457 | 35.554 | 1134.698 | 65.000× | 0.491× |
| `alert-early` / 4,096 | 66.731 | 38.270 | 1383.678 | 20.735× | 1.744× |
| `within-early` / 4,096 | 62.721 | 38.253 | 1326.722 | 21.153× | 1.640× |
| `latest-recent` / 4,096 | 22.344 | 35.088 | 1155.824 | 51.729× | 0.637× |

### Empty controls

| Batch / linking | Imre ms | Rust ms | CPython ms | Largest Imre control/workload fraction |
|---|---:|---:|---:|---:|
| full-normal | 2.664 | 2.579 | 24.354 | 14.81% |
| full-thinlto | 2.310 | 2.408 | 24.100 | 15.73% |
| early-normal | 2.485 | 2.439 | 24.445 | 7.74% |
| early-thinlto | 2.819 | 2.415 | 23.976 | 16.83% |

Across all conditions, maximum empty-control/workload fractions are 16.83% for Imre, 24.72% for Rust, and 3.06% for CPython. Controls are not subtracted. Empty inputs omit nonempty parsing/alignment, so this control does not estimate all fixed cost. The ratios include process startup, parsing, alignment, queries and final output; warm caches and no `fsync` apply.

## CPU and allocations

The six profiles use the exact timed ThinLTO binaries and `alert-quiet-4096`: Callgrind at 128 queries, DHAT at 128 and 256 queries, for both native engines. Profiles run after all wall timing and pass the complete independent output/filesystem oracle. Callgrind counts guest instructions rather than CPU cycles; DHAT counts requested allocation bytes/blocks rather than RSS. Two-point allocation slopes do not prove behavior for other inputs.

| Engine | Guest instructions, 128 queries | Allocated bytes, 128 queries | Allocation blocks | Additional bytes / query, 128→256 | Additional blocks / query |
|---|---:|---:|---:|---:|---:|
| Imre | 75,409,166 | 7,100,646 | 168,152 | 40 | 1 |
| Rust | 29,401,993 | 2,656,472 | 41,065 | 0 | 0 |

Whole-process Imre guest instructions changed from 162,095,513 in exploratory candidate 003 to 89,422,181 in 005 and 75,409,166 in the final build (+53.48% reduction relative to 003). The final complete-process instruction ratio is 2.565× Rust. These observations do not assign wall-time savings to individual checks or transformations.

## Exploratory trajectory and historical context

The exploratory candidates are separate six-sample, one-warmup pilots with four 4,096-record workloads and an empty control. They guided optimization; final acceptance uses the twelve-sample matrix above. Each stage has its own interleaved Rust baseline, so changes across stages are contextual rather than an isolated causal estimate. The retained audits contain both native medians, not only their ratio.

| Stage | Report / Rust | Alert / Rust | Within / Rust | Latest / Rust |
|---|---:|---:|---:|---:|
| Historical original ThinLTO | 8.460× | 13.438× | 13.246× | 8.851× |
| Pilot 003 | 5.368× | 8.877× | 8.680× | 6.702× |
| Pilot 005 | 1.910× | 2.785× | 2.820× | 2.500× |
| Pilot 006 | 1.779× | 2.637× | 2.634× | 2.265× |
| Pilot 007 | 1.664× | 2.431× | 2.409× | 2.193× |
| Pilot 008 | 1.641× | 2.530× | 2.328× | 2.045× |
| Pilot 009 | 1.363× | 2.184× | 2.164× | 1.874× |
| Pilot 010 | 1.387× | 1.745× | 1.704× | 1.432× |
| Final | 1.385× | 1.723× | 1.672× | 1.409× |

The separate candidate-005 branch simulation recorded 5,992,704 conditional branches and 299,668 simulated misses in `fn_7`; 299,521 misses were attributed to the verified `jle` at `0x10aeb3`. This motivated the pure absolute-value transformation. Those are simulated predictor events, not hardware counters, and no final speedup is attributed to them alone. The exploratory probe did not recapture every profiler tool hash at probe time; that limitation remains in its audit.

An earlier candidate-006 pilot attempt stopped before timing when the coactivity guard detected another build workload; its run list is empty and it contributes no samples. The completed candidate-006 pilot shown above used a subsequent plan. A separate independent audit verifies that five mutable Ohm tool paths were rebound to retained files with the exact original hashes. Historical records stay unchanged. Final full validation uses a task-local Rustup home pointing to those retained tools; its before/after identity matches the archived setup record. The original tool-identity baseline, relocation audit and aborted status are retained as provenance.

## Validation and evidence

The exact frozen implementation passed **138 full-validation jobs** with **224 Rust unit tests**. All 28 language suites ran under ordinary O0/O3, ThinLTO O3, and ASan/LSan O0/O1. Candidate compiler/runtime builds have fresh-invocation evidence. The benchmark reuses the unchanged, source-bound Rust reference binaries from renamed validation v2. Both exact timed binary pairs passed the full telemetry oracle during preparation, including huge integers, overflow, invalid input, alignment errors, aliases, hard links, output-device failure and complete file effects.

The final audit verified **2,898 process executions**: 138 correctness gates, 1,104 warmups and 1,656 measured executions. Every complete stdout/stderr/status/filesystem result and balanced measured order was checked. Source, helper, tool, build, input and binary hashes are retained; timing and profiling use the same source revision with no post-validation source patch.

The [rename closure](2026-10-21-iterator-performance-rename-closure.json) separately records preserved repository/Stack identities and all 56 CI-enabled renamed heads passing, with 2 workflow-free heads. That closure covers the pre-performance stack; later PR CI is separate.

Read the [independent final audit](2026-10-21-iterator-performance-audit.json), [full-validation audit](2026-10-21-iterator-performance-validation-audit.json), [profile summary](2026-10-21-iterator-performance-profile.json), and [evidence manifest](2026-10-21-iterator-performance-evidence-manifest.json). The [deterministic evidence archive](2026-10-21-iterator-performance-evidence.tar.gz) retains exact Git source, commands, preparation/oracle logs, raw timing outputs, profile data and reproduction helpers. Native binaries, toolchains, build caches, Git administrative data and previous archives are excluded by recorded type/hash/reason; no private rename inventory is published.

| Raw complete measurement record | Uncompressed JSON SHA256 |
|---|---|
| [full-normal](2026-10-21-iterator-performance-full-normal.json.gz) | `84190e0bd30bd82c108739662d0b6e64dcd9a8f356ddfdce664766be81885f5d` |
| [full-thinlto](2026-10-21-iterator-performance-full-thinlto.json.gz) | `11004c01e98a75a9d39c892ef41129d92697f994346ed3d538e3db6cd3c6be15` |
| [early-normal](2026-10-21-iterator-performance-early-normal.json.gz) | `fbcfed4d1c1d938b219327e3f07eeaa9b15ee8f93623840a66f5993301296266` |
| [early-thinlto](2026-10-21-iterator-performance-early-thinlto.json.gz) | `30c5a5054ed8d7e1c312eaaa84c63f579ae93b9fae7b5b2590d36e2d7e66c49c` |

### Exact measurement commands

The commands below describe the retained machine paths. The archive contains the exact sources and helper scripts; reproduction must select equivalent local paths and rerun all identity checks.

```sh
/home/dev-user/.local/share/uv/python/cpython-3.12.13-linux-x86_64-gnu/bin/python3.12 -I -B /tmp/imre-telemetry-final-evidence.py measure --plan /tmp/imre-telemetry-final-v1/plan.json --phase full
```

```sh
/home/dev-user/.local/share/uv/python/cpython-3.12.13-linux-x86_64-gnu/bin/python3.12 -I -B /tmp/imre-telemetry-final-evidence.py measure --plan /tmp/imre-telemetry-final-v1/plan.json --phase early
```

The canonical planned engine/preparation argv, tool versions, optimization flags and target triples are preserved in the linked records. CPython/Clang identities and Python/Rust program sources match the historical input baseline; native Rust binaries are pinned to renamed validation v2; this is one machine and workload, not a portable performance guarantee.
