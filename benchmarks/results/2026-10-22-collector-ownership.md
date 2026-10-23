# Ruff’s diagnostic collector within 2× Rust

Relar compared and sorted Ruff’s 249 diagnostic entries in 268.0 µs with ordinary linking and 215.4 µs with ThinLTO. This is 1.45× and 1.51× the time of the unchanged native Rust collector. The corresponding #95 builds took 829.6 µs and 541.9 µs. All seven implementations matched every title, test-case key and source, in order. [Ruff collector](../../examples/conformance_report/render.py) · [Previous comparison](2026-10-22-entry-sort.md)

## Collector

Microseconds per complete collection of 1,269 preloaded groups. The harness times zero and 256 rounds in separate processes. Every round exchanges two cases, executes the application’s collector and consumes every row in order. A full transcript checks exact field values independently of the timed checksum.

| Workload | Link | Upstream CPython | Typed CPython | #93 port | #95 Relar | Relar | Codon | Rust | Relar/Rust |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Collect entries | Ordinary | 2,082.28 | 1,907.24 | 860.37 | 829.58 | 267.96 | 1,601.09 | 185.39 | 1.45× |
| Collect entries | ThinLTO | 2,115.19 | 1,938.67 | 543.15 | 541.88 | 215.43 | 1,585.26 | 142.70 | 1.51× |

A separate 512-pass run interleaved ordinary Relar, ThinLTO Relar and both corresponding Rust builds on CPU 31. It measured 1.44× and 1.53× Rust across eight balanced rounds.

## Where the allocations went

Ruff compares old and new diagnostics by mapping each diagnostic to a six-field fingerprint tuple, sorting both projected lists, and checking corresponding tuples. Of 1,011 groups with equal nonempty lengths, 972 contained a single diagnostic on each side. Another 37 contained two on each side and two contained three. Eighteen groups with two populated sides exited at the unequal-length check. [Python diagnostic equivalence](../../examples/conformance_report/model.py)

When each input has one element and the mapper only reads stored fields, Relar compares those fields directly. Two empty inputs return the reduction’s empty result. The collector’s generated singleton path executes four string comparisons and two Python-integer comparisons with no record allocation, retain or release calls. Longer inputs take the existing sort and compare path. Direct synchronous calls to statically known map/filter callbacks avoid heap-allocating a callable environment. [Compiler proof](../../crates/relar-compiler/src/frontend/projected_equality.rs) · [Pipeline lowering](../../crates/relar-compiler/src/frontend/pipelines.rs)

Ordinary native builds, subtracting zero rounds from 64 collector rounds under Callgrind and DHAT. Allocation events and bytes count cumulative heap traffic.

| Implementation | Instructions/collection | Allocations/collection | Allocated bytes/collection |
|---|---:|---:|---:|
| #95 Relar | 7,313,108 | 10,963 | 388,617 |
| Relar | 2,124,692 | 1,008 | 55,952 |
| Rust | 1,665,013 | 2,822 | 224,663 |

The observed diagnostic equivalence call stacks accounted for 10,192 allocations per pass in #95 and 238 after the optimization. The archive contains the allocation counts and byte totals for each observed stack.

## Complete, repeated and live reports

Milliseconds per report. Complete starts a fresh process on captures. Repeated subtracts zero-pass startup from four passes. Live discovers the checkout and runs both real ty binaries. Each complete output file matches the vendored Ruff implementation byte-for-byte.

| Workload | Link | Upstream CPython | Typed CPython | #93 port | #95 Relar | Relar | Codon | Rust | Relar/Rust |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Unchanged GitHub · complete | Ordinary | 145.85 | 280.47 | 36.48 | 36.22 | 35.60 | 62.13 | 23.58 | 1.51× |
| Unchanged GitHub · repeated | Ordinary | 52.11 | 171.76 | 17.73 | 17.21 | 16.64 | 32.14 | 10.47 | 1.59× |
| Unchanged GitHub · live | Ordinary | 969.04 | 1,308.56 | 883.68 | 882.50 | 881.43 | 958.23 | 872.48 | 1.01× |
| Unchanged GitHub · complete | ThinLTO | 146.10 | 278.88 | 32.13 | 32.52 | 31.28 | 60.83 | 21.83 | 1.43× |
| Unchanged GitHub · repeated | ThinLTO | 50.49 | 171.33 | 13.53 | 13.61 | 12.95 | 31.88 | 9.02 | 1.44× |
| Unchanged GitHub · live | ThinLTO | 979.73 | 1,325.61 | 882.06 | 888.80 | 881.75 | 960.50 | 872.11 | 1.01× |
| Changed GitHub · complete | Ordinary | 147.20 | 277.23 | 37.52 | 36.84 | 35.86 | 61.72 | 23.95 | 1.50× |
| Changed GitHub · repeated | Ordinary | 51.33 | 170.28 | 18.46 | 18.11 | 17.58 | 34.14 | 11.24 | 1.56× |
| Changed GitHub · live | Ordinary | 1,002.53 | 1,337.27 | 911.09 | 908.15 | 904.11 | 990.80 | 891.01 | 1.01× |
| Changed GitHub · complete | ThinLTO | 147.11 | 279.49 | 33.19 | 33.21 | 32.54 | 62.29 | 22.42 | 1.45× |
| Changed GitHub · repeated | ThinLTO | 52.01 | 172.58 | 14.41 | 14.56 | 13.97 | 34.37 | 9.92 | 1.41× |
| Changed GitHub · live | ThinLTO | 998.91 | 1,349.46 | 913.54 | 909.47 | 904.61 | 989.07 | 895.66 | 1.01× |
| Changed diff · complete | Ordinary | 149.93 | 282.98 | 37.78 | 37.03 | 36.40 | 63.17 | 24.15 | 1.51× |
| Changed diff · repeated | Ordinary | 50.69 | 170.17 | 17.80 | 17.52 | 16.97 | 34.04 | 11.03 | 1.54× |
| Changed diff · live | Ordinary | 983.31 | 1,317.06 | 892.92 | 897.04 | 900.26 | 973.69 | 883.24 | 1.02× |
| Changed diff · complete | ThinLTO | 146.92 | 276.99 | 32.47 | 32.57 | 31.72 | 61.69 | 22.13 | 1.43× |
| Changed diff · repeated | ThinLTO | 51.64 | 171.61 | 13.83 | 14.10 | 13.58 | 34.33 | 9.53 | 1.43× |
| Changed diff · live | ThinLTO | 994.94 | 1,330.90 | 894.29 | 902.29 | 900.84 | 983.72 | 880.39 | 1.02× |

The largest full-report ratio, including complete, live, four-pass totals and per-report compute, was 1.62× Rust. The serial native control measured 1.41–1.58× across complete and repeated reports.

## Measurement and replay

Candidate `b12109581c90c5608fb52c26bcf6eb3e4c7adece`, #95 baseline `53172f1c02e282b51af8436262ea38fb5751b1ae`, and #93 port `490ff91eb4ac3d9af47e790259a7441bf8a6fa00`. The frozen #93 port and #95 compiler/runtime each use their own published source. #95 and the candidate compile the same Python reporter and timed collector driver. Both modes reuse the original Rust and Codon binaries with verified source and executable hashes. Fetch the pull refs and select these immutable commits to reproduce the measured revisions. [Original Ruff source](../../examples/conformance_report/upstream/conformance.py) · [Codon adaptations](../codon_native/README.md)

The 146-file typing checkout was `835b70f3f795f8e061b1f7621178515b0a77a0ad`. Ordinary and ThinLTO ran on physical cores 31 and 15 of the same AMD EPYC-Milan Processor, with separate L1/L2 and a shared guest L3. Each mode ran seven engines serially for fourteen rounds with balanced positions and directed neighbors, after one warmup per engine. The formal matrices contain 2,744 measured processes and 196 warmups. The separate CPU 31 control adds 352 measured processes and 44 warmups. Tools were Clang 18.1.3, GNU ld 2.42, LLD 22.1.8, Ohm Rust 1.98.1-1, CPython 3.14.5 and Codon 0.20.4. [Measurement driver](../conformance_regex_benchmark.py) · [Independent auditor](../conformance_regex_audit.py)

Extract the [portable study](2026-10-22-collector-ownership-study.tar.xz) into an empty directory and run `python3.14 -B replay.py` there. It retains the source and build receipts, 146 inputs, exact output oracles, raw CPU and heap traces, per-sample wall times, output hashes and frozen validation receipts, including 322 Rust unit tests and 37 suites run at O0 and O3 with and without sanitizers. [Ordinary samples](2026-10-22-collector-ownership-ordinary.json.gz) · [ThinLTO samples](2026-10-22-collector-ownership-thinlto.json.gz) · [Audit receipt](2026-10-22-collector-ownership-audit.json)
