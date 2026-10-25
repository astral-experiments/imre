> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Generator expressions and telemetry comparison

Imre now compiles immediately consumed generator expressions, `zip` (including `strict=True`), `reversed`, `any`, and `all`. The [telemetry comparator](../../examples/telemetry_comparison/README.md) runs the same typed Python package on CPython and Imre, with an independent Rust implementation of the same file protocol and queries.

At 4,096 samples with ThinLTO, the full-scan group runs **4.48–5.84× faster than CPython** and takes **8.46–13.44× Rust's time**. Early-exit queries run **12.81–19.40× faster than CPython** and take **1.55–2.91× Rust's time**. These are complete-program measurements, not isolated iterator instruction costs.

The [implementation PR](https://github.com/viarius-experiments/imre/pull/57) continues the existing GitHub stack. This milestone adds functionality and measures its cost; the remaining Rust gap is visible in the results.

## Supported behavior

Generator expressions can compose nested clauses and filters with the supported builtin iterators when consumed directly by loops, eager comprehensions, `list`, exact-integer `sum`, `any`, or `all`. Construction and iterator acquisition happen at the required time; pulls, filters, exceptions, live captures, and strict-length checks remain lazy. `any` and `all` stop at the first decisive value. Reverse list iteration observes mutation, and strings reverse by Unicode code point.

The compiler lowers these pipelines to typed local state. It can remove private destructured zip/enumerate tuples without allocating a generator closure. Tuples that transitively own files preserve CPython's observable iterator-cache ownership and cleanup behavior. General generator functions, escaping iterator values, and user-defined iterator protocols remain outside the supported subset; see the [subset reference](../../docs/subset.md).

## Workload and comparison

The program reads reference and observed timestamp/value recordings, validates complete timestamp alignment using strict zip, then answers repeated queries. `report-mixed` makes separate signed, absolute, and squared-error reductions, two threshold predicates, and a reverse search. Quiet predicates and searches scan the complete input. Early/recent cases stop after one inspected pair. Complete alignment is validated once before queries, so an early result cannot hide a missing sample.

All engines retain arbitrary-precision integer results and the same errors. The independent Rust reference uses contiguous native records and checked `i128` reductions when values fit, restarting a pure query with `BigInt` on overflow. Imre uses the typed Python object's storage and generated loops. Rust applies `black_box` around every repeated query and result; all engines render the final complete result once and return a repetition checksum.

Full-scan processes target **8,388,608 source-level inspected pairs**; early-exit processes target **2,097,152**. Repetition counts round down to whole queries. Input sizes are 16, 256, and 4,096 records per file. The raw records retain each exact input, pair count, repetition count, output, and filesystem result.

Each condition has eight warmup rounds and twelve measured rounds. All six engine permutations occur twice in measured rounds, balancing positions and within-round directed predecessors. Cross-round transitions and the eight warmup rounds are not fully balanced.

## Wall-time results

Values are median milliseconds. **CPython / Imre** is a speedup (higher is better); **Imre / Rust** is a time ratio (lower is better). Raw samples and inclusive quartiles are retained in the linked records.

### Ordinary linking

| Workload / records | Imre ms | Rust ms | CPython ms | CPython / Imre | Imre / Rust |
|---|---:|---:|---:|---:|---:|
| `report-mixed` / 16 | 868.458 | 26.712 | 1324.274 | 1.525× | 32.511× |
| `alert-quiet` / 16 | 819.448 | 21.170 | 1232.438 | 1.504× | 38.708× |
| `within-quiet` / 16 | 825.097 | 23.436 | 1231.605 | 1.493× | 35.207× |
| `latest-quiet` / 16 | 774.215 | 20.556 | 994.877 | 1.285× | 37.664× |
| `report-mixed` / 256 | 770.430 | 18.207 | 821.886 | 1.067× | 42.315× |
| `alert-quiet` / 256 | 741.473 | 10.331 | 970.155 | 1.308× | 71.774× |
| `within-quiet` / 256 | 762.776 | 11.042 | 963.059 | 1.263× | 69.078× |
| `latest-quiet` / 256 | 751.379 | 12.948 | 804.179 | 1.070× | 58.030× |
| `report-mixed` / 4,096 | 773.167 | 21.260 | 791.825 | 1.024× | 36.367× |
| `alert-quiet` / 4,096 | 749.348 | 13.836 | 965.333 | 1.288× | 54.160× |
| `within-quiet` / 4,096 | 772.909 | 14.633 | 981.497 | 1.270× | 52.820× |
| `latest-quiet` / 4,096 | 764.557 | 15.973 | 804.177 | 1.052× | 47.866× |
| `alert-early` / 16 | 370.063 | 48.262 | 1348.852 | 3.645× | 7.668× |
| `within-early` / 16 | 345.154 | 48.760 | 1305.396 | 3.782× | 7.079× |
| `latest-recent` / 16 | 287.136 | 36.180 | 1133.011 | 3.946× | 7.936× |
| `alert-early` / 256 | 372.789 | 48.983 | 1371.309 | 3.679× | 7.611× |
| `within-early` / 256 | 349.500 | 50.024 | 1316.680 | 3.767× | 6.987× |
| `latest-recent` / 256 | 289.059 | 36.255 | 1146.706 | 3.967× | 7.973× |
| `alert-early` / 4,096 | 379.040 | 53.207 | 1377.012 | 3.633× | 7.124× |
| `within-early` / 4,096 | 355.717 | 53.098 | 1330.149 | 3.739× | 6.699× |
| `latest-recent` / 4,096 | 291.736 | 40.263 | 1142.341 | 3.916× | 7.246× |

### ThinLTO

| Workload / records | Imre ms | Rust ms | CPython ms | CPython / Imre | Imre / Rust |
|---|---:|---:|---:|---:|---:|
| `report-mixed` / 16 | 203.186 | 19.884 | 1299.883 | 6.398× | 10.219× |
| `alert-quiet` / 16 | 181.773 | 15.149 | 1231.137 | 6.773× | 11.999× |
| `within-quiet` / 16 | 180.139 | 15.627 | 1238.757 | 6.877× | 11.527× |
| `latest-quiet` / 16 | 137.905 | 18.271 | 988.264 | 7.166× | 7.548× |
| `report-mixed` / 256 | 170.413 | 18.185 | 804.384 | 4.720× | 9.371× |
| `alert-quiet` / 256 | 164.604 | 10.085 | 970.954 | 5.899× | 16.322× |
| `within-quiet` / 256 | 162.299 | 9.987 | 954.026 | 5.878× | 16.251× |
| `latest-quiet` / 256 | 132.316 | 13.354 | 806.305 | 6.094× | 9.909× |
| `report-mixed` / 4,096 | 178.110 | 21.053 | 798.667 | 4.484× | 8.460× |
| `alert-quiet` / 4,096 | 169.025 | 12.578 | 966.220 | 5.716× | 13.438× |
| `within-quiet` / 4,096 | 168.478 | 12.719 | 967.081 | 5.740× | 13.246× |
| `latest-quiet` / 4,096 | 140.340 | 15.856 | 819.688 | 5.841× | 8.851× |
| `alert-early` / 16 | 101.437 | 33.852 | 1379.095 | 13.596× | 2.997× |
| `within-early` / 16 | 95.650 | 35.086 | 1329.046 | 13.895× | 2.726× |
| `latest-recent` / 16 | 53.778 | 35.290 | 1115.900 | 20.750× | 1.524× |
| `alert-early` / 256 | 101.413 | 35.222 | 1360.225 | 13.413× | 2.879× |
| `within-early` / 256 | 96.558 | 32.642 | 1318.997 | 13.660× | 2.958× |
| `latest-recent` / 256 | 53.960 | 32.586 | 1126.796 | 20.882× | 1.656× |
| `alert-early` / 4,096 | 107.540 | 37.793 | 1377.344 | 12.808× | 2.845× |
| `within-early` / 4,096 | 102.550 | 35.274 | 1314.425 | 12.817× | 2.907× |
| `latest-recent` / 4,096 | 59.311 | 38.248 | 1150.781 | 19.402× | 1.551× |

Ordinary and ThinLTO families have separate interleaved baselines. Native engines use matching linking modes; ThinLTO uses matched LLVM import and inline budgets of 1,000. Differences between families do not isolate the effect of linking alone.

### Startup controls

| Batch / linking | Imre ms | Rust ms | CPython ms |
|---|---:|---:|---:|
| full / normal | 2.387 | 2.510 | 24.432 |
| full / thinlto | 2.297 | 2.304 | 24.030 |
| early / normal | 2.804 | 2.656 | 24.381 |
| early / thinlto | 2.257 | 2.307 | 24.396 |

The empty-recording, one-query controls are reported without subtraction. The largest control/workload median fractions are 4.20% for Imre, 24.30% for Rust, and 3.09% for CPython. The pilot used smaller batches; the final batch sizes were selected before final measurement to reduce startup's contribution.
The empty control excludes nonempty parsing and alignment, so it is not an estimate of all fixed costs. Those operations remain included in every workload measurement. Rust startup remains material, so these ratios should not be presented as isolated steady-state throughput.

## CPU and allocation profile

Profiles use the exact timed ThinLTO executables on `alert-quiet-4096`, with 128 queries (524,288 inspected pairs). Callgrind and DHAT run separately from wall-time measurements; a second DHAT run uses 256 queries. All six executions pass the complete output and filesystem oracle.

| Engine | Guest instructions | Allocated bytes, 128 queries | Allocation blocks | Additional bytes / query, 128→256 |
|---|---:|---:|---:|---:|
| Imre | 230,051,902 | 7,100,626 | 168,152 | 40 |
| Rust | 29,380,573 | 2,656,473 | 41,065 | 0 |

The generated alert query (`fn_7`) accounts for 46.86% of Imre self instructions; `imre_seq_get` accounts for another 23.43%, with 1,056,768 calls. Concrete class fields already use direct loads. The query still performs generic sequence access, bounds and exception checks, record ownership handoffs, and tagged-integer arithmetic around each pair. Borrowed sequence access and stronger integer proofs are candidates for later specialization; the emitted LLVM does not assign a measured cost to each individual check. Callgrind counts guest instructions, not CPU cycles or wall-time shares.

Doubling queries adds 40 bytes and one allocation per extra Imre query, and no allocations for Rust. This two-point observation suggests that most allocation work is in loading and setup, while repeated per-pair operations dominate the query gap; it is not a proof of allocation behavior for other inputs. The Rust reference also spends substantial profile work parsing and loading. Both profiles include the complete process, so the instruction ratio is not an isolated query-loop ratio.

See the [profile summary](2026-10-20-iterator-pipelines-profile.json), [independent profile audit](2026-10-20-iterator-pipelines-profile-audit.json), and raw Callgrind/DHAT data in the evidence archive.

## Validation and provenance

The frozen compiler/runtime revision `547c7252cde4b0e5e4b1eae3f39670c71d902989` passed **138 validation jobs** and **191 Rust unit tests**. All 28 language suites ran with ordinary O0/O3, ThinLTO O3, and ASan/LSan O0/O1. The iterator suite passed 75, 61, and 75 checks respectively, including 47 compile-time rejections per build mode. Seven application oracles and six reference oracles passed. The telemetry oracle compares 258 cases plus output-device failure, including malformed input, overflow fallback, huge integers, timestamp/length errors, aliases, hard links, and complete filesystem effects.

Timing used clean revision `13ed29910b6f52df7959a37afd12ad46b1e5d638`. Its only changes after full validation fix target verification in the benchmark harness and explain that verification in the reference guide. Compiler, runtime, examples, validators, tests, and Rust reference source are unchanged. The exact patch is retained. Both exact timed binary pairs passed the full telemetry oracle again during preparation. A later metadata-only correction names input counts `records` so they cannot be overwritten by measured `samples` round lists. These original records are unchanged; their labels and retained inputs establish the input counts.

Compiler, runtime, and telemetry reference builds have raw evidence of fresh compiler invocations. Other regression reference builds are source-bound invocations with preserved artifact hashes; some reused their build cache. The independent validation audit distinguishes them.

An unchanged-Luhn control compares emitted LLVM from the previous and current compiler. Every byte is identical after removing only the two new, unused runtime declarations. This checks generated-code stability; it is not a new Luhn wall-time measurement.

Measurements ran pinned to CPU 0, outside ptrace, with task-owned builds, tests, profiles, and large evidence audits stopped. The host is AMD EPYC-Milan; CPython 3.14.5 has PGO/LTO, the GIL enabled, and JIT disabled. Imre uses Clang 18 at O3; Rust uses Ohm 1.98.1-1, rustc 1.98.1-dev, and LLVM 22.1.8. Shared-host activity and frequency remain uncontrolled.

Timing includes process startup, imports, parsing and complete input alignment once, repeated queries, checksum accumulation, final output, flush/close, and exit. Fixture creation, verification, compilation, and hashing are outside the clock. OS caches are warm and output is not fsynced.

The final records retain **2,898 executions: 1,656 measured, 1,104 warmups, and 138 correctness gates**, including four empty-input controls. Every execution checks complete stdout, stderr, status, and filesystem contents. The independent audit recomputes summaries and ratios, checks order balance, and binds results to exact source, tool, build, validation, and binary identities.

Raw records: [full/normal](2026-10-20-iterator-pipelines-full-normal.json.gz), [full/thinlto](2026-10-20-iterator-pipelines-full-thinlto.json.gz), [early/normal](2026-10-20-iterator-pipelines-early-normal.json.gz), [early/thinlto](2026-10-20-iterator-pipelines-early-thinlto.json.gz). See the [validation audit](2026-10-20-iterator-pipelines-validation-audit.json), [benchmark audit](2026-10-20-iterator-pipelines-benchmark-audit.json), [evidence archive](2026-10-20-iterator-pipelines-evidence.tar.gz), and [member manifest](2026-10-20-iterator-pipelines-evidence-manifest.json).

The archive preserves measured sources, LLVM, logs, complete raw results, the exploratory pilot, audits, and reproduction helpers. Native binaries, runtime archives, interpreters, and toolchains remain external and hash-identified. Prior evidence archives are not nested. Original commands retain host-specific paths; the [example guide](../../examples/telemetry_comparison/README.md) and [Rust reference guide](../reference/telemetry_comparison/README.md) provide portable entrypoints.

The [independent archive audit](2026-10-20-iterator-pipelines-archive-audit.json) verifies all 1,515 members, 54 exclusions, and the frozen Git source snapshots. Its [auditor](2026-10-20-iterator-pipelines-archive-audit.py) is retained separately because the archive was already finalized.
