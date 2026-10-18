> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Typed comprehensions and inventory reconciliation

Relar now compiles eager typed list, dictionary, and set comprehensions, including nested `for` clauses and ordered filters. This is the next capability milestone because it builds directly on existing iteration, containers, concrete layouts, and exception cleanup. It makes filtering, flattening, and constructing typed collections practical in ordinary application code.

The new inventory reconciler runs unchanged on CPython and Relar. With matched ThinLTO, its two 16,384-positive-request workloads run **1.79–2.24× faster than CPython** and take **1.56–1.57× Rust BigInt's time**. Ordinary builds run **1.29–1.58× faster than CPython** and take **1.90–2.00× Rust's time**. Rust remains faster on these substantial workloads. The unchanged incremental-planner controls have overlapping interquartile ranges and median changes below 0.9% in both modes.

The compiler implementation is `615aa4175280a2a0cedf7f549720a2578e246d4e`. The application, Rust reference, oracle, benchmark harness, and clean measured checkout are `edbbab93b26ccca638c7bf74336baef0d0fab4cd`. The tool build predates the application commit and records its then-untracked files; every compiled source hash matches both commits. The preserved control executable comes from the [concrete-layout comparison](2026-10-18-concrete-layouts.md), whose implementation is `6d0db50ab7851f4321704e6f97b79a2d92d58606`. The compiler and example continue the existing PR stack in [PR 25](https://github.com/viarius-experiments/relar/pull/25) and [PR 26](https://github.com/viarius-experiments/relar/pull/26).

## Capability and semantics

Comprehensions lower to private typed helpers using the existing loops and container operations. The first iterable is evaluated in the enclosing scope; subsequent iterables, filters, and result expressions execute in order inside the comprehension's scope. Targets do not leak, and later target names are reserved throughout that scope. Dictionary keys evaluate before values, repeated keys replace values while retaining first insertion position, and filters can narrow optional types for later clauses.

Captures use the existing borrowed-argument rules. Partially constructed results and iterator state follow ordinary exception cleanup. Mutation checks, arbitrary-precision integers, exact container types, and constructor initialization checks remain in force. Literal signed range bounds retain existing small-integer range proofs where valid. The [subset reference](../../docs/subset.md) gives the supported sources, typing rules, and restrictions; generators, asynchronous comprehensions, and assignment expressions remain unsupported.

This milestone changes the frontend and its semantic tests. It adds no runtime representation, ABI, tracing GC, or closure environment. Rebuilt ordinary, ThinLTO, and sanitizer runtime archives are byte-identical to their concrete-layout counterparts. The unchanged planner's generated LLVM IR is also byte-identical in all four old/new linking-mode artifacts; executable hashes are recorded separately and differ.

## The application

The [inventory reconciler](../../examples/inventory_reconciliation/README.md) reads stock and batched orders, fulfills positive requests in input order against shared mutable stock, and writes a complete report. List comprehensions parse nested batches, flatten/filter requests, construct fulfillment records, and select shortages. Dict comprehensions preserve first appearance while building last-wins stock and initializing requested totals; a following loop accumulates those totals. A set comprehension identifies requested SKUs, and a list comprehension uses membership to select untouched stock. Set traversal never determines report order.

Quantities and totals have arbitrary precision. Ignored nonpositive requests retain their original row positions; unknown SKUs never create stock entries. Both input files are read and closed before output opens, including when paths or hard links alias. Malformed input leaves an existing report untouched. Output errors are terminal, and partial output is permitted. The application guide defines the shared Unicode, numeric, path, ordering, and error domain precisely.

The [Rust reference](../reference/inventory_reconciliation/README.md) follows the same eager algorithm with BigInt arithmetic, ordered maps, sequential fulfillment, and checked file closure. Rust uses native containers, borrowed strings, and direct formatting, so allocation representations differ. This comparison measures the complete implementations of the same program, including those differences.

The independent oracle does not import the application. It derives fulfillment from initial stock minus cumulative earlier demand and checks conservation, complete report bytes, final error category/message, exit status, and filesystem contents. It covers 176 filesystem cases: 127 successes and 49 terminal failures, plus a separate Linux `/dev/full` case for each engine.

| Workload | Stock rows / unique SKUs | Batches | All / positive requests | Report bytes |
|---|---:|---:|---:|---:|
| empty | 0 / 0 | 0 | 0 / 0 | 215 |
| boundary-128 | 4 / 3 | 3 | 130 / 128 | 36,498 |
| dense-2048 | 65 / 64 | 10 | 2,364 / 2,048 | 81,626 |
| sparse-2048 | 2,080 / 2,048 | 10 | 2,364 / 2,048 | 170,989 |
| dense-16384 | 65 / 64 | 85 | 18,906 / 16,384 | 654,942 |
| sparse-16384 | 16,640 / 16,384 | 85 | 18,906 / 16,384 | 1,438,871 |

Dense cases repeat 64 stock SKUs. Sparse cases include many unique and unknown SKUs. Both retain ignored requests and empty batches. The boundary case includes quantities around `10**80`, duplicate stock, Unicode names, and unknown requests.

## Measurement method

Each mode measures six inventory workloads with CPython, Relar, and Rust BigInt, and separately compares five unchanged incremental workloads with CPython, preserved Relar, and new Relar. The recorded execution order is controls followed by inventory. Every workload has 30 measured rounds containing all six engine permutations five times, plus two warmups per engine. Positions and directed within-round predecessors are balanced; cross-round predecessors and workload order are not. Across both modes, all **1,980 measured executions and 132 warmups** passed complete output checks, with no discarded samples.

Every sample starts a fresh process pinned to CPU 0. Inventory runs use fresh temporary directories. Timing includes startup/imports, input open/read/decode/parse, eager collection construction, fulfillment, full report rendering and writes, explicit flush/close, and exit. Builds, oracle execution, fixture preparation, comparison/hashing, and deletion are outside the timer. Inputs and executable pages are warm, output is not fsynced, and CPython uses its normal warmed bytecode cache. Full validation precedes timing; the session record verifies the 12 application/control package cache headers against source and preserves identical before/after cache hashes. It does not inventory the entire Python standard-library cache.

Ordinary and ThinLTO sessions ran sequentially outside ptrace after the task's builds, tests, and profilers stopped. The records retain affinity, runtime-relevant environment, and three zero-TracerPid checkpoints per mode. CPU frequency and shared machine resources are uncontrolled. These are whole-process measurements on one shared devbox; they do not isolate comprehension execution or storage throughput. The independent sample audit recomputes statistics from integer nanoseconds and checks recorded outputs, order, provenance, and sample completeness. It is not an independent operating-system process log.

## Inventory results

Times are median milliseconds with observed Q1–Q3 in brackets. These ranges describe sample spread, not confidence intervals. Ratios divide medians; `Relar/Rust` above one means Rust is faster.

### Ordinary linking

| Input | CPython [Q1–Q3] | Relar [Q1–Q3] | Rust BigInt [Q1–Q3] | CPython/Relar | Relar/Rust |
|---|---:|---:|---:|---:|---:|
| empty | 26.99 [26.76–27.41] | 2.19 [2.13–2.26] | 2.26 [2.17–2.34] | 12.33× | 0.97× |
| boundary-128 | 27.98 [27.76–28.37] | 3.03 [2.95–3.10] | 2.70 [2.62–2.87] | 9.23× | 1.12× |
| dense-2048 | 33.07 [32.74–33.91] | 8.15 [8.01–8.45] | 5.98 [5.87–6.08] | 4.06× | 1.36× |
| sparse-2048 | 36.56 [36.29–37.10] | 12.65 [12.24–12.84] | 8.35 [8.28–8.48] | 2.89× | 1.51× |
| dense-16384 | 72.49 [72.06–73.35] | 45.99 [45.41–46.43] | 23.03 [22.87–23.24] | 1.58× | 2.00× |
| sparse-16384 | 104.39 [103.41–105.51] | 80.64 [77.71–81.07] | 42.37 [42.12–42.74] | 1.29× | 1.90× |

### ThinLTO

| Input | CPython [Q1–Q3] | Relar [Q1–Q3] | Rust BigInt [Q1–Q3] | CPython/Relar | Relar/Rust |
|---|---:|---:|---:|---:|---:|
| empty | 26.71 [26.48–27.27] | 2.12 [2.09–2.23] | 2.14 [2.10–2.20] | 12.60× | 0.99× |
| boundary-128 | 29.37 [28.70–30.00] | 3.09 [2.88–3.36] | 2.74 [2.63–3.06] | 9.50× | 1.13× |
| dense-2048 | 33.85 [32.97–34.09] | 7.23 [7.08–7.48] | 5.66 [5.55–5.81] | 4.68× | 1.28× |
| sparse-2048 | 36.06 [35.91–36.23] | 10.01 [9.95–10.08] | 7.63 [7.60–7.68] | 3.60× | 1.31× |
| dense-16384 | 74.34 [73.76–75.77] | 33.17 [32.89–33.87] | 21.27 [20.97–21.73] | 2.24× | 1.56× |
| sparse-16384 | 106.26 [104.18–107.48] | 59.28 [58.77–59.94] | 37.83 [37.38–38.66] | 1.79× | 1.57× |

Empty and boundary cases include a substantial startup and small-file floor, but these measurements do not isolate those costs; their large CPython ratios do not establish a similar computational speedup. The empty Relar/Rust ranges overlap. ThinLTO helps the large native application, but the two modes were separate sessions, so the report uses within-session engine comparisons as its main result. No C benchmark was performed.

## Unchanged-program controls

These are the exact incremental-planner sources and captured inputs from the concrete-layout report. `Change` is `new / preserved − 1`; negative is faster. The generated IR identity and overlapping ranges support the conclusion that comprehensions introduce no observed material regression in these existing programs.

| Mode / input | CPython | Preserved Relar [Q1–Q3] | New Relar [Q1–Q3] | Change |
|---|---:|---:|---:|---:|
| Ordinary / layered-16384-sparse | 113.57 | 50.32 [49.92–50.60] | 50.24 [50.05–50.81] | -0.16% |
| Ordinary / layered-16384-all | 194.79 | 74.86 [74.01–75.63] | 75.07 [74.11–76.39] | +0.27% |
| Ordinary / wide-16384-sparse | 102.57 | 26.28 [26.18–26.60] | 26.48 [26.38–26.74] | +0.76% |
| Ordinary / wide-16384-all | 186.23 | 44.52 [44.13–44.95] | 44.69 [44.05–45.05] | +0.38% |
| Ordinary / boundary-128-all | 21.09 | 3.66 [3.59–3.76] | 3.65 [3.60–3.74] | -0.14% |
| ThinLTO / layered-16384-sparse | 112.48 | 42.29 [41.79–42.84] | 42.31 [41.92–43.06] | +0.03% |
| ThinLTO / layered-16384-all | 191.97 | 63.85 [63.02–65.32] | 63.57 [62.55–64.25] | -0.44% |
| ThinLTO / wide-16384-sparse | 102.17 | 21.51 [21.22–21.82] | 21.47 [21.29–21.65] | -0.17% |
| ThinLTO / wide-16384-all | 186.83 | 35.96 [35.41–36.53] | 35.64 [35.26–36.06] | -0.89% |
| ThinLTO / boundary-128-all | 20.77 | 3.51 [3.47–3.55] | 3.48 [3.46–3.57] | -0.63% |

Some individual control samples have substantial latency outliers, including a 275 ms preserved-native sample in ordinary layered/all. Their causes were not measured, and those samples remain in the raw records. Sub-percent median movements are not claimed as compiler speedups.

## Validation and tools

The final compiler/runtime pass formatting, Clippy, **25 compiler and 36 runtime unit tests**, and **1,778 native semantic/CLI harness checks**: 706 ordinary O0/O3 checks, 531 ThinLTO O3 checks, 531 AddressSanitizer O1 checks, and 10 ThinLTO CLI checks. These totals comprise 701 native executions and 1,077 compiler/CLI rejection checks; they exclude additional CPython references and successful compiler subprocesses. The new comprehension suite contributes 65/52/52 checks: 39 rejection checks in each mode and 26/13/13 native runtime comparisons. Its 13 runtime fixtures also agree on CPython 3.12 and 3.14.

The eight suites cover integers, imports, classes, containers, exceptions, interactive/output I/O, files/arguments, and comprehensions. Tests exercise target scope, outer and inner iterable evaluation, dictionary key/value order, mutable captures, optional narrowing, iteration mutation, partial-result cleanup, constructor rules, recursion/effects, and large range boundaries. Sanitizer runs instrument generated code and the Rust runtime/target dependencies, with leak detection outside ptrace; the prebuilt Rust standard library is not instrumented.

Both timing modes pass the full inventory oracle for all three engines and all 172 existing incremental graph/parser cases for their three control engines before measurement. Each inventory workload also gets a full untimed gate. A separately built, fully instrumented inventory application passes the 176-case oracle and device failure with leak detection, using the same final compiler/runtime sources.

The host is Linux x86-64 on AMD EPYC-Milan. CPython 3.14.5 has the GIL enabled, JIT disabled, and recorded PGO/LTO configuration. Relar uses Clang 18.1.3 and Rust 1.98.1-dev (`f62703110`, Ohm `ohm-1.98.1-1`, LLVM 22.1.8). ThinLTO uses LLD 22.1.8 and import/inline budgets of 1000 for both native implementations. Backend versions and language/runtime implementations still differ. Inventory AOT compilation takes 0.621 seconds ordinary and 6.745 seconds with ThinLTO, excluding compiler/runtime builds and all execution timings.

## Evidence and reproduction

- [Ordinary raw records](2026-10-18-comprehensions-normal.json.gz) and [ThinLTO raw records](2026-10-18-comprehensions-lto.json.gz) contain complete inputs/expectations, every sample, ordering, source identities, and frozen reference build records.
- [Independent audit and script](2026-10-18-comprehensions-audit.json), [validation index](2026-10-18-comprehensions-validation.json), and [compiler/runtime build snapshot](2026-10-18-comprehensions-build.json).
- [Sequential session and bytecode-cache record](2026-10-18-comprehensions-session.json), [source/build/validation evidence archive](2026-10-18-comprehensions-evidence.tar.gz), and [archive member/hash manifest](2026-10-18-comprehensions-evidence-manifest.json).

The application and Rust-reference guides give build, oracle, and timing commands. The session record preserves the exact two benchmark invocations and runner source. Use separately frozen compiler/runtime/reference artifacts with matching records; pass `--validate-only` to run all correctness gates without timing. Reproducing the optional historical control requires its preserved executable and original layout record. Rebuilt binaries need their own truthful provenance records. The archive includes source snapshots, generated LLVM IR, exact local commands, logs, and audit scripts; executable/runtime binaries remain local and are identified by SHA256.

The timing gzip files preserve their original JSON bytes with `mtime=0`. Decompressed SHA256 values are `6cc110d2813388208bed649a01bfe7bd9ddba2fffeda9d459dad15312a629dae` (ordinary) and `6e9cf00a5d185e5ac52e698520c584b01364fa38baf77a9e51c0d8ca977ab96c` (ThinLTO). Local originals are retained.
