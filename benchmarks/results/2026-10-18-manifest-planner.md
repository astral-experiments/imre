> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# File-based first-party AOT programs

Relar now supports a complete file-driven command-line application: the [manifest dependency planner](../../examples/manifest_planner/README.md) reads UTF-8 request files, recovers from malformed or missing inputs, and writes a complete report. On the large workloads, matched ThinLTO takes **139.94–140.46 ms** for layered graphs and **77.60–79.01 ms** for wide graphs. That is **1.86–3.27× faster than CPython**, while taking **1.72–1.79× Rust BigInt's time**. Rust remains faster.

The five unchanged incremental workloads move by less than 1% versus the preserved [exception-optimized compiler](2026-10-18-exception-performance.md) in both linking modes. Their observed interquartile ranges overlap. This supports a small impact on these workloads; it is not a universal zero-overhead claim.

The measured source is clean commit `35f07cbeac120b7631cb38b049224bb98b4b25d5`. The implementation remains an independent Rust runtime with LLVM AOT compilation and Ruff/ty infrastructure.

## Functionality and semantics

The expansion adds sealed `typing.TextIO`, explicit UTF-8 read/write `open`, `read`, `write`, `flush`, `close`, `closed`, and builtin-file `with`. It also adds one shared mutable `sys.argv` list, scalar `str` conversion, and Python-compatible no-argument `splitlines`. The [subset contract](../../docs/subset.md#utf-8-files-and-process-arguments) states the accepted forms and error hierarchy.

File aliases share state. Explicit close marks the handle closed even when flushing or closing fails. A hidden owner keeps each `with` manager alive through return, break, continue, reassignment, and exception propagation; nested managers close in reverse order. Final reference release performs best-effort resource cleanup without installing a pending exception. Files are leaves in the ownership graph and use the existing reference-counted runtime. Programs that do not read `sys.argv` emit no argument initialization or cleanup calls.

The manifest program reuses the existing batch payload parser and incremental graph algorithm. Each path is resolved from the process working directory. Input failures become normalized request results; scheduling and report failures remain terminal. It reads the manifest before truncating the report and flushes each request header before opening a payload, making report/payload aliases and hard links deterministic. The checked-in sample completes two requests, recovers from one malformed request, and closes all three payload attempts.

The Rust BigInt reference uses the same parser and algorithm with native containers and `Result` recovery. It explicitly flushes and checks Linux file closure, including cleanup after errors. Integer arithmetic remains arbitrary precision in all three implementations. This compares the supported observable behavior, rather than CPython traceback text or exception internals.

## Measurement method

Each row is the median of 30 fresh processes after two warmups per engine and workload. Every three-engine group repeats all six engine orders five times. Positions and directed adjacent pairs within a round are balanced; cross-round transitions are not. The harness and children are pinned to CPU 0, and traced timing is refused. Ordinary and ThinLTO runs execute sequentially after all other builds, tests, and profilers stop.

Every file execution gets a fresh isolated working directory. Fixture creation and input writes, independent oracle work, complete filesystem comparisons, hashing, and deletion are outside the timer. Process startup, Python imports, file opens and reads, parsing, graph validation, scheduling, report writes, flush/close, and exit are inside. Inputs are created immediately before timing and are page-cache warm. Output is not fsynced. The local filesystem reports `ext2/ext3` with 4096-byte blocks. These are whole-program warm-cache measurements, not cold-storage or durable-write throughput.

Large manifests contain two valid 16,384-target all-changed payloads. Mixed variants add an invalid integer, a missing file, and invalid UTF-8 after each valid request: two valid and six recovered requests. The 128-target boundary workload exercises large integers and startup costs. The new program bulk-reads payloads and writes report files, so its ratios should not be interpreted as an exception-performance delta against the prior stdin/stdout batch program. The unchanged control group provides the compiler/runtime comparison.

All 1,980 measured executions and 132 warmups passed full output and filesystem checks. The independent raw-data audit also rechecked 2,214 embedded oracle executions, all engine ordering, captured input identity, source and artifact hashes, and every median and ratio. No samples were discarded. One ordinary CPython wide-valid sample took 762 ms; its median was 253 ms. Small boundary workloads show substantial native timing spread, especially mixed cases. Interquartile ranges below describe observed spread, not confidence intervals.

## File-workload results

Times are milliseconds. `CPython/Relar` is a speed ratio; `Relar/Rust` is an elapsed-time ratio, so values above one mean Rust is faster.

### Ordinary linking

| Input | CPython | Relar [Q1–Q3] | Rust BigInt | CPython/Relar | Relar/Rust |
|---|---:|---:|---:|---:|---:|
| layered-16384-all-valid | 263.67 | 163.38 [161.91–166.51] | 89.67 | 1.61× | 1.82× |
| layered-16384-all-mixed | 262.12 | 163.01 [161.31–165.36] | 89.33 | 1.61× | 1.82× |
| wide-16384-all-valid | 252.95 | 96.33 [95.35–97.09] | 47.28 | 2.63× | 2.04× |
| wide-16384-all-mixed | 251.53 | 96.29 [95.29–97.04] | 47.46 | 2.61× | 2.03× |
| boundary-128-all-valid | 29.07 | 3.32 [3.25–3.43] | 2.69 | 8.76× | 1.23× |
| boundary-128-all-mixed | 29.45 | 4.15 [3.58–4.71] | 2.90 | 7.10× | 1.43× |

### ThinLTO

| Input | CPython | Relar [Q1–Q3] | Rust BigInt | CPython/Relar | Relar/Rust |
|---|---:|---:|---:|---:|---:|
| layered-16384-all-valid | 260.59 | 139.94 [138.48–140.94] | 81.21 | 1.86× | 1.72× |
| layered-16384-all-mixed | 260.71 | 140.46 [138.92–143.21] | 81.48 | 1.86× | 1.72× |
| wide-16384-all-valid | 258.14 | 79.01 [78.20–79.64] | 44.53 | 3.27× | 1.77× |
| wide-16384-all-mixed | 252.88 | 77.60 [76.75–78.03] | 43.39 | 3.26× | 1.79× |
| boundary-128-all-valid | 29.24 | 3.27 [3.14–4.37] | 2.73 | 8.95× | 1.20× |
| boundary-128-all-mixed | 29.67 | 4.48 [3.39–4.58] | 2.92 | 6.63× | 1.53× |

Relar's large mixed and valid medians differ by less than 2%. Their differences do not isolate the cost of an exception or file operation. This milestone adds functionality; it does not establish Rust parity or identify the remaining performance gap's cause. No C comparison was performed.

## Unchanged-program controls

These use the exact five captured inputs and unchanged incremental Python sources from the exception-performance record. The preserved executable is the optimized pre-file build. Both executables run during the same balanced session; older published wall times are not substituted. Change is `new / preserved − 1`.

### Ordinary linking

| Input | CPython | Preserved Relar | New Relar [Q1–Q3] | Change |
|---|---:|---:|---:|---:|
| layered-16384-sparse | 113.88 | 58.70 | 59.26 [58.89–60.33] | +0.94% |
| layered-16384-all | 192.24 | 87.63 | 87.67 [86.94–88.88] | +0.04% |
| wide-16384-sparse | 104.67 | 29.47 | 29.56 [29.00–30.21] | +0.31% |
| wide-16384-all | 186.35 | 51.20 | 50.85 [50.37–51.39] | -0.68% |
| boundary-128-all | 20.89 | 3.61 | 3.61 [3.59–3.64] | -0.07% |

### ThinLTO

| Input | CPython | Preserved Relar | New Relar [Q1–Q3] | Change |
|---|---:|---:|---:|---:|
| layered-16384-sparse | 113.83 | 51.83 | 52.31 [51.87–53.46] | +0.93% |
| layered-16384-all | 192.31 | 76.83 | 76.96 [76.56–77.37] | +0.17% |
| wide-16384-sparse | 102.25 | 24.14 | 24.19 [23.96–24.40] | +0.22% |
| wide-16384-all | 186.10 | 42.18 | 41.94 [41.34–42.49] | -0.58% |
| boundary-128-all | 20.89 | 3.53 | 3.53 [3.49–3.57] | +0.05% |

The median changes range from −0.684% to +0.943% ordinary and −0.576% to +0.935% with ThinLTO. These small differences overlap the observed variation and are not claimed as speedups or regressions.

## Validation, toolchain, and limits

The independent manifest oracle checks all file bytes and directory entries across 196 cases, including Unicode/newlines, parser and graph errors, invalid UTF-8, missing/directory/NUL paths, repeated recovery, output aliases, hard links, and terminal failures. It covers 199 completed parses, 92 recovered invalid requests, and nine terminal cases. A separate Linux `/dev/full` case verifies that report-write errors remain terminal and preserve inputs. CPython, ordinary Relar/Rust, and ThinLTO Relar/Rust all pass. Each control engine also passes the existing 155 graph cases and 17 parser cases.

All seven native semantic suites pass with ordinary O0/O3, ThinLTO O3, and AddressSanitizer O1: files/arguments, exceptions, containers, integers, imports, classes, and I/O. The new file suite reports 72 ordinary checks and 51 each for ThinLTO/ASan, including descriptor-exhaustion cleanup and a synchronized broken-pipe test. The fully instrumented manifest application passes all 196 cases and `/dev/full`; both generated code and the Rust runtime/target dependencies use AddressSanitizer, with leak detection enabled outside ptrace. The prebuilt Rust standard library is not instrumented. The record also includes 19 compiler and 36 runtime unit tests, formatting, Clippy, and 10 ThinLTO CLI checks. Earlier workspace logs preserve outcomes but not their exact command lines; the evidence marks that limitation explicitly.

The host is Linux x86-64 on AMD EPYC-Milan. CPython is 3.14.5, with the GIL enabled and JIT disabled; its recorded build configuration uses PGO/LTO. Relar uses Clang 18.1.3 and Rust 1.98.1-dev (`f62703110`, Ohm `ohm-1.98.1-1`, LLVM 22.1.8). Both native ThinLTO configurations use LLVM import/inline budgets of 1000. Ordinary Rust uses the recorded no-LTO release profile. The frontend, runtime, and backend details still differ. This shared devbox has uncontrolled frequency and shared-resource noise despite CPU affinity and balanced ordering.

Application AOT compilation takes 0.750 s ordinary and 6.908 s with ThinLTO, excluding compiler/runtime builds; neither is included in execution times. The manifest executables are 3,283,640 / 3,154,440 bytes, and generated LLVM IR is 224,699 bytes. The unchanged control executable grows from 3,152,616 to 3,275,392 bytes ordinary and from 2,752,968 to 2,810,488 bytes with ThinLTO, despite its similar measured runtime.

This is a bounded file API: strict UTF-8, read-all and buffered write, explicit modes/encoding, and sealed builtin-file context management. General context-manager protocols, binary/append/seek APIs, arbitrary standard-library modules, and surrogateescaped arguments remain outside this milestone. Invalid native argument bytes raise `UnicodeDecodeError` on first `sys.argv` access; they are outside the common CPython/Rust benchmark input domain.

## Evidence and reproduction

- [Ordinary raw samples, gzip](2026-10-18-manifest-planner-normal.json.gz) and [ThinLTO raw samples, gzip](2026-10-18-manifest-planner-thinlto.json.gz) retain complete inputs, expected changed file bytes, all per-run inventory hashes/statuses/times, oracle checks, tool identities, and Rust build records.
- [Independent audit and audit script](2026-10-18-manifest-planner-audit.json), [compiler/runtime build and exact source record](2026-10-18-manifest-planner-build.json), [sanitizer build record](2026-10-18-manifest-planner-sanitizer-build.json), and [full native/application validation evidence](2026-10-18-manifest-planner-validation.json).

The gzip files preserve the original JSON bytes and use `mtime=0`. Decompressed SHA256 values are `7914155d5a259e71ababad1871f833c1495b8e1676c6e01ae3335cd9b5852959` (ordinary) and `485df73cabc3fb19c63557b98f260c195dcc5d116f93bccfa421e3e33a7a6237` (ThinLTO). Local originals were retained.

The [example guide](../../examples/manifest_planner/README.md#correctness-and-comparison) provides the sample and exact comparison invocation. Build at the recorded source commit with the normal pinned toolchain, or the documented local Ohm wrapper, and retain a compiler/runtime build record. Build the [Rust manifest reference](../reference/incremental_planner/README.md) with `--manifest`, selecting matching ordinary/ThinLTO mode and LLVM budget. Supply the preserved exception-optimized control executable and its original comparison record; the harness rejects changed sources, mismatched binaries, targets, and modes. Rebuilding a baseline requires a new corresponding baseline record rather than modifying saved hashes.

Run `manifest_benchmark.py` with 30 samples, two warmups, and CPU 0 after other work stops, outside ptrace. For ThinLTO select `librelar_runtime_lto.a` and an actual compatible LLD backend named `ld.lld`; preserve that lexical symlink name. The raw comparison and build records retain compilation commands, program commands, build environment, source hashes, full input bytes, and the filesystem timing boundary.
