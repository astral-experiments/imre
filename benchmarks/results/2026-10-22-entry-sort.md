# Ruff diagnostic entries and complete reports

Relar compiles Ruff’s original `collect_diagnostic_entries` function, including `list.sort(key=...)` and the two `typing.assert_never` branches. The 146-file typing suite produces 1,269 groups and 249 ordered diagnostic entries. All six implementations matched the title, test-case key and source of every row, and the complete bytes of each report. [Reporter](../../examples/conformance_report/README.md) · [Pinned Ruff source](../../examples/conformance_report/upstream/conformance.py)

## Diagnostic entries

Microseconds per collection of all 249 rows, after subtracting the zero-round preload. Each process prepares both real ty captures and groups the diagnostics. Each of 128 subsequent rounds exchanges the first and last case, calls the actual collector and consumes the ordered positions, lengths of the title and key, and source. A separate run compares the complete ordered transcript.

| Workload | Build | Upstream CPython | Typed CPython | #93 Relar | Relar | Codon | Rust | Relar/Rust |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Collect and sort | Ordinary | 2,089.34 | 1,901.55 | 861.74 | 830.70 | 1,607.34 | 187.81 | 4.42× |
| Collect and sort | ThinLTO | 2,122.80 | 1,925.50 | 544.50 | 551.72 | 1,608.63 | 150.99 | 3.65× |

The isolated collector took 4.42× Rust’s time in ordinary release and 3.65× under ThinLTO. Against typed CPython, Relar was 2.29× and 3.49× faster.

## Reports

Milliseconds per report. Complete starts a fresh CLI on captured ty output. Repeated subtracts a zero-pass control and divides a four-report computation by four. Live starts a fresh CLI, discovers the checkout and runs both pinned ty binaries. All three panels use the original Ruff implementation as a byte-for-byte oracle.

| Workload | Build | Upstream CPython | Typed CPython | #93 Relar | Relar | Codon | Rust | Relar/Rust |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Unchanged GitHub · complete | Ordinary | 147.59 | 277.17 | 36.62 | 35.40 | 60.73 | 23.72 | 1.49× |
| Unchanged GitHub · repeated | Ordinary | 49.43 | 169.66 | 17.26 | 16.85 | 31.58 | 10.22 | 1.65× |
| Unchanged GitHub · live | Ordinary | 968.42 | 1,307.08 | 884.91 | 884.80 | 954.87 | 864.38 | 1.02× |
| Unchanged GitHub · complete | ThinLTO | 146.65 | 278.25 | 31.98 | 31.78 | 60.56 | 21.95 | 1.45× |
| Unchanged GitHub · repeated | ThinLTO | 50.44 | 169.80 | 13.24 | 13.53 | 31.80 | 8.62 | 1.57× |
| Unchanged GitHub · live | ThinLTO | 973.87 | 1,311.64 | 878.97 | 879.64 | 958.42 | 865.62 | 1.02× |
| Changed GitHub · complete | Ordinary | 145.74 | 276.05 | 37.04 | 36.37 | 61.43 | 23.95 | 1.52× |
| Changed GitHub · repeated | Ordinary | 51.47 | 170.72 | 18.51 | 18.19 | 33.83 | 11.43 | 1.59× |
| Changed GitHub · live | Ordinary | 1,009.77 | 1,330.48 | 906.10 | 903.82 | 987.17 | 893.68 | 1.01× |
| Changed GitHub · complete | ThinLTO | 147.47 | 280.81 | 33.00 | 32.61 | 62.00 | 22.48 | 1.45× |
| Changed GitHub · repeated | ThinLTO | 51.35 | 171.47 | 14.29 | 14.37 | 34.03 | 9.71 | 1.48× |
| Changed GitHub · live | ThinLTO | 1,001.19 | 1,333.33 | 892.45 | 898.40 | 982.71 | 886.10 | 1.01× |
| Changed diff · complete | Ordinary | 147.69 | 278.14 | 36.93 | 36.47 | 61.77 | 24.08 | 1.51× |
| Changed diff · repeated | Ordinary | 50.53 | 170.58 | 17.71 | 17.29 | 33.62 | 10.88 | 1.59× |
| Changed diff · live | Ordinary | 978.57 | 1,312.13 | 893.60 | 889.87 | 973.89 | 873.40 | 1.02× |
| Changed diff · complete | ThinLTO | 147.23 | 279.05 | 32.82 | 32.59 | 61.68 | 22.52 | 1.45× |
| Changed diff · repeated | ThinLTO | 51.83 | 171.50 | 13.74 | 13.97 | 34.15 | 9.46 | 1.48× |
| Changed diff · live | ThinLTO | 1,006.29 | 1,330.57 | 895.31 | 900.44 | 982.95 | 886.07 | 1.02× |

The largest complete, repeated or live report ratio was 1.66× Rust. The gate includes the four-pass process totals and their preload-subtracted per-pass measurements. A separate serial comparison put both modes on CPU 31 alone. Across three complete and repeated reports, it measured 1.44–1.67× Rust over eight balanced rounds.

## CPU and allocation profiles

The ordinary builds ran 64 passes under Callgrind and DHAT. The table subtracts the zero-pass control and rounds per-collection counts to the nearest whole number.

| Engine | Instructions/collection | Allocations/collection | Allocated bytes/collection |
|---|---:|---:|---:|
| #93 Relar | 7,361,147 | 10,970 | 392,681 |
| Relar | 7,313,345 | 10,963 | 388,617 |
| Rust | 1,664,773 | 2,822 | 224,663 |

The new collector executed 0.65% fewer instructions and made seven fewer allocations per call than the #93 port. Record release, allocator bookkeeping, and `malloc` were its three largest self-costs in Callgrind. The full report also parses diagnostics, groups cases, and renders Markdown.

## Measurement and replay

Timed source `918e77728b7afdadb5ab1d63f785cb2c291482b8`. The #93 baseline is `490ff91eb4ac3d9af47e790259a7441bf8a6fa00`. Its tree matches the #93 squash commit on main. The pinned typing checkout is `835b70f3f795f8e061b1f7621178515b0a77a0ad`.

The six engines ran serially within each mode on an AMD EPYC-Milan Processor with CPython 3.14.5 and Codon 0.20.4. Ordinary and ThinLTO ran concurrently on separate physical cores (31 and 15) with private L1/L2 and a shared guest L3. Both native modes used Clang 18.1.3 for Relar and Ohm Rust 1.98.1-1 for the frozen Rust build. Ordinary used GNU ld 2.42. ThinLTO used LLD 22.1.8. Codon used the same release compilation flags in both modes and linked the matching native platform library. Output hashes were checked on every repetition. Each six-engine panel used a warmup followed by twelve rounds with balanced positions and directed neighbors. [Native Codon adapters](../codon_native/README.md) · [Benchmark driver](../conformance_regex_benchmark.py) · [Ordered-entry driver](../conformance_classification.py)

Extract the [portable study](2026-10-22-entry-sort-study.tar.xz) into an empty directory and run `python3.14 -B replay.py` there. It includes frozen sources, commands, hashes, LLVM IR, raw profiles, 146 test files, captures, complete per-engine outputs and independent oracle checks. The included local QA archive covers 320 Rust unit tests, 37 suites at O0 and O3 with and without sanitizers, and all 6,110 records in the upstream uv sort check. [Ordinary samples](2026-10-22-entry-sort-ordinary.json.gz) · [ThinLTO samples](2026-10-22-entry-sort-thinlto.json.gz) · [Audit receipt](2026-10-22-entry-sort-audit.json)
