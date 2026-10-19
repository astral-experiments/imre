> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Floats, bytes, pattern matching, and assignment expressions

Relar now compiles all four capabilities. A binary measurement parser exercises them together; the unchanged inventory reconciler checks performance against CPython, preserved Relar, and Rust BigInt. On the two large inventory workloads, ThinLTO Relar runs **2.70–3.56× faster than CPython** and takes **6.1–10.1% more time than Rust BigInt**.

[Floats](https://github.com/viarius-experiments/relar/pull/37), [bytes](https://github.com/viarius-experiments/relar/pull/38), [matching](https://github.com/viarius-experiments/relar/pull/39), and [assignment expressions](https://github.com/viarius-experiments/relar/pull/40) continue GitHub Stack 4 after the [sequence-access milestone](2026-10-18-sequence-access.md). Measurements and final validation use source commit `97cb1000565035162c31e6e455d207cf65b54993`; this report and README refresh are committed separately.

## Capability and example

- **Floats:** binary64 arithmetic, exact mixed integer/float comparisons, numeric conversions, scalar formatting, and concrete class/container storage. Integer true division rounds the ratio once, even when neither integer fits in a float. Ordinary floats use unboxed LLVM doubles without fast-math; present `float | None` payloads currently use an allocation to preserve every binary64 bit pattern.
- **Bytes:** immutable literals and values, indexing, iteration, slicing, concatenation/repetition, scalar formatting, strict UTF-8 conversion, bytes dictionary/set keys, and typed binary files with read/write/append/exclusive-create modes.
- **Matching:** scalar/singleton patterns, captures, wildcards, OR/as patterns, guards, tuple/list patterns with starred rest, dictionary patterns with `**rest`, and keyword fields on declared classes. Subjects evaluate once; successful captures survive a false or raising guard. Numeric patterns compare exact values across integers, floats, and booleans.
- **Assignment expressions:** `:=` stores a local and yields its original right-hand value. Evaluation order, optional narrowing, owned values, and definite assignment account for those writes. Eager comprehension bindings reach the enclosing function or entry scope while iteration targets remain private.

The [integrated example](../../tests/fixtures/assignments/integrated_scalars.py) reads a binary measurement file, decodes records, matches list and typed-class patterns, accumulates optional float readings with `:=`, and writes a binary summary. It also checks rebinding, guards, signed zero, large numeric patterns, and malformed byte-number conversions. Its stdout and both output files agree with CPython under ordinary, ThinLTO, and sanitizer configurations.

The [language contract](../../docs/subset.md) describes the exact boundary. Float keys/set elements, exponentiation, complex numbers, general codecs, dynamic value patterns, positional class patterns, and protocol dispatch remain unsupported. List and tuple membership/equality also remain unsupported; rejection probes check these NaN identity-sensitive operations. Relar's exact checker validates operator, assignment, argument, and return compatibility, including dead code and unused functions; ty continues to provide other semantic diagnostics. Functions containing `:=` conservatively skip range/small-integer proofs and tuple-producer scalarization until those optimizations account for rebinding.

## Inventory comparisons

All six inventory source files are byte-identical to the preceding milestone. The Rust source and executable references are reused unchanged after verification. The comparison therefore holds application source fixed while changing Relar's compiler/runtime. It measures existing-program performance after this expansion; throughput of the new features requires separate workloads.

Times are median milliseconds. Positive change means more time than the preserved sequence-access Relar executable. Ratios use per-engine medians.

**Ordinary linking**

| Input | CPython ms | Previous Relar ms | Current Relar ms | Rust BigInt ms | Relar time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.102 | 2.215 | 2.203 | 2.247 | -0.53% |
| boundary-128 | 28.302 | 2.876 | 2.853 | 2.658 | -0.81% |
| dense-2048 | 33.540 | 6.542 | 6.543 | 6.023 | +0.02% |
| sparse-2048 | 37.484 | 9.165 | 9.311 | 8.435 | +1.59% |
| dense-16384 | 76.915 | 26.898 | 27.288 | 23.049 | +1.45% |
| sparse-16384 | 111.205 | 50.603 | 51.265 | 42.579 | +1.31% |

**ThinLTO**

| Input | CPython ms | Previous Relar ms | Current Relar ms | Rust BigInt ms | Relar time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.374 | 2.237 | 2.231 | 2.240 | -0.23% |
| boundary-128 | 28.307 | 2.775 | 2.838 | 2.682 | +2.27% |
| dense-2048 | 33.490 | 5.683 | 5.726 | 5.572 | +0.76% |
| sparse-2048 | 37.305 | 7.926 | 7.975 | 7.705 | +0.63% |
| dense-16384 | 76.863 | 21.373 | 21.583 | 20.349 | +0.98% |
| sparse-16384 | 111.025 | 40.856 | 41.159 | 37.385 | +0.74% |

Across twelve comparisons, median changes range from **−0.81% to +2.27%**. The four large-workload comparisons show small median increases of **0.74–1.45%**. Every previous/current Relar interquartile range overlaps. This capability milestone makes no native speedup claim. The [summary](2026-10-19-floats-bytes-match-walrus-summary.json) retains exact statistics; interquartile ranges describe sample spread rather than confidence intervals or proof of equivalent performance.

For 16,384 positive requests, ordinary Relar runs 2.17–2.82× faster than CPython and takes 18.4–20.4% more time than Rust BigInt. ThinLTO narrows the Rust gap to 6.1–10.1%. Small cases include substantial process-startup and small-file overhead. Generated LLVM differs from the preserved milestone in both modes. No new CPU or allocation profiles were collected.

## Method and provenance

The session ran on October 19, 2026, from 03:30:00 to 03:31:46 UTC. Each mode uses six captured workloads, 48 measured rounds covering all 24 engine permutations twice, and two warmup rounds. Measured execution positions and directed within-round predecessors are balanced; warmups, cross-round predecessors, and workload order are not. Records retain **2,304 measured executions, 96 warmups, and 48 workload gates**.

Before timing, each mode checks the independent 204-case oracle and `/dev/full` on all four engines: **1,632 oracle outcomes and eight device-error outcomes**. Every timed run and warmup checks exit status, stdout/stderr, complete report bytes, and filesystem effects. The launcher verifies clean source, frozen tools, preserved references, and unchanged CPython bytecode caches before and after the session.

Fresh processes run on CPU 0 outside ptrace. Timing includes startup/imports, reading/decoding/parsing, reconciliation, rendering, buffered writes, flush/close, and exit. Builds, fixture preparation, oracles, hashing, verification, and deletion stay outside the clock. OS caches are warm; writes are not fsynced. Task-owned builds, tests, and profiling stop during measurements; CPU frequency and unrelated activity on the shared host remain uncontrolled.

The host is x86-64 AMD EPYC-Milan. CPython 3.14.5 is a GIL build with the experimental JIT disabled; its recorded configuration enables PGO/LTO. Relar uses Clang 18.1.3 at `-O3`, with LLD 22.1.8 for cross-language ThinLTO and matched LLVM optimization budgets. The Rust BigInt reference preserves the eager algorithm and arbitrary-precision quantities, using native containers, borrowed strings, and direct formatting. These results apply to this complete inventory program and recorded environment.

## Validation and evidence

All **3,409 semantic/regression checks** pass: **1,113 ordinary**, **851 ThinLTO plus ten ThinLTO CLI checks**, and **1,435 ASan/LSan**. Counts include compile-time rejections. They cover the four new features together with existing imports, classes, containers, comprehensions, calls, formatting, sequence access, exceptions, I/O, and files. The final source also passes **81 Rust unit tests**, Clippy with warnings denied, formatting, and focused Ruff checks.

After timing, the inventory application separately passes all **204 oracle cases and `/dev/full`** under O1 ASan/LSan outside ptrace, with leak detection enabled. All frozen source/tool hashes remain unchanged. Sanitizers instrument generated code and runtime dependencies; the prebuilt Rust standard library remains uninstrumented.

Twenty-three initial suite invocations failed during temporary-fixture preparation because copied directories inherited read-only permissions. Restoring writable fixture directories without changing their contents allowed all affected invocations to pass. Original setup failures and successful retry records remain in the evidence archive; no compiler or sanitizer failures occurred in these runs.

The [validation index](2026-10-19-floats-bytes-match-walrus-validation.json) joins the final source, tested compiler/runtimes, receipts, and logs. [Build](2026-10-19-floats-bytes-match-walrus-build.json) and [session](2026-10-19-floats-bytes-match-walrus-session.json) records retain commands, source hashes, artifact identities, and actual timestamps. Runtime artifacts compiled earlier are reconciled to the final commit through their captured source hashes.

The [independent timing audit](2026-10-19-floats-bytes-match-walrus-audit.json) checks raw integer-nanosecond samples, execution orders, outcomes, and provenance. [Ordinary](2026-10-19-floats-bytes-match-walrus-normal.json.gz) and [ThinLTO](2026-10-19-floats-bytes-match-walrus-lto.json.gz) records preserve original JSON bytes under deterministic gzip. The [evidence archive](2026-10-19-floats-bytes-match-walrus-evidence.tar.gz) and [manifest](2026-10-19-floats-bytes-match-walrus-evidence-manifest.json) retain source, generated LLVM, raw records, validation logs, and reproduction/audit scripts. Native binaries, runtime archives, interpreters, toolchains, and bytecode caches are excluded. A portable publication audit checks retained text/source evidence; rerunning the complete live artifact audit requires the original native artifacts identified in the records.
