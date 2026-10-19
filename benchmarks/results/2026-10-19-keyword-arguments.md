> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Keyword arguments and scalar defaults: inventory reconciliation

Keyword calls, keyword-only parameters, and scalar defaults now work in the inventory reconciler with **byte-identical LLVM and native `.text` in ordinary and ThinLTO builds**. Across twelve comparisons, current Relar median times differ from the preserved executable by −1.51% to +0.56%; every previous/current interquartile range overlaps. This is a capability and readability milestone, with no native speedup claim.

[PR 33](https://github.com/viarius-experiments/relar/pull/33) continues GitHub Stack 4 after the [scalar f-string milestone](2026-10-18-scalar-fstrings.md). Measurements use clean commit `3b7377367a6c93b19aa8981ccd97131bc7442e83`. Five application modules adopt named arguments, meaningful keyword-only parameters, and the existing `"stock"` label as an omitted default. Supplied expressions retain their previous order; the algorithm, inputs, outputs, and Rust implementation are unchanged.

## Supported calls

First-party functions, instance methods, and constructors accept positional and named arguments. Binding respects positional-only `/` and keyword-only `*` parameters, including imported aliases and re-exports. Defaults are exact `int`, `bool`, `str`, or `None` literals; integers may have a single unary sign and use arbitrary-precision decimal, hexadecimal, octal, or binary notation.

```python
def load(path: str, *, label: str = "stock") -> list[str]:
    ...

stock_lines = load(stock_path)
order_lines = load(order_path, label="orders")
```

The receiver and supplied arguments evaluate once in source order, even when keywords reorder parameters. A failure stops later arguments and the call. Exact types, contextual container/optional types, and ownership across mutation or failure remain checked. Binding is resolved during compilation and lowers to ordinary native calls.

Missing, duplicate, unexpected, or wrongly typed arguments are compile-time errors. Variadic parameters, `*args`/`**kwargs` expansion, mutable or computed defaults, and default f-strings are rejected. Builtins retain their existing conventions; this does not add general keyword support to `print`, container methods, or file methods. See the [language contract](../../docs/subset.md#function-arguments-and-defaults).

## Results

Times are median milliseconds. Positive Relar change means more time than the previous executable. The [complete summary](2026-10-19-keyword-arguments-summary.json) retains exact medians, Q1/Q3, ratios, and overlap checks. Interquartile ranges describe sample spread, not confidence intervals.

**Ordinary**

| Input | CPython ms | Previous Relar ms | Current Relar ms | Rust BigInt ms | Relar time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.118 | 2.171 | 2.183 | 2.228 | +0.56% |
| boundary-128 | 27.800 | 2.752 | 2.756 | 2.568 | +0.14% |
| dense-2048 | 33.147 | 6.373 | 6.362 | 5.887 | -0.17% |
| sparse-2048 | 37.255 | 9.134 | 9.088 | 8.337 | -0.50% |
| dense-16384 | 76.435 | 26.975 | 26.623 | 22.900 | -1.31% |
| sparse-16384 | 110.552 | 51.029 | 50.259 | 42.139 | -1.51% |

**ThinLTO**

| Input | CPython ms | Previous Relar ms | Current Relar ms | Rust BigInt ms | Relar time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.118 | 2.111 | 2.104 | 2.157 | -0.33% |
| boundary-128 | 28.678 | 2.791 | 2.769 | 2.632 | -0.79% |
| dense-2048 | 33.728 | 5.791 | 5.762 | 5.666 | -0.50% |
| sparse-2048 | 37.132 | 8.006 | 7.977 | 7.790 | -0.36% |
| dense-16384 | 76.647 | 21.404 | 21.320 | 20.389 | -0.39% |
| sparse-16384 | 111.090 | 41.063 | 40.983 | 37.339 | -0.19% |

For the two 16,384-positive-request cases, ThinLTO Relar runs **2.71–3.60× faster than CPython** and takes **4.6–9.8% more time than Rust BigInt**. Ordinary Relar runs 2.20–2.87× faster than CPython and takes 16.3–19.3% more time than Rust. Small inputs include a substantial startup and small-file floor.

The [read-only native audit](2026-10-19-keyword-arguments-native-text.json) verifies all 1,032,601 ordinary `.text` bytes and 1,040,928 ThinLTO `.text` bytes match the baseline. Whole ELF files differ only in symbol strings and, for ThinLTO, the build-ID note. No new CPU or allocation profiles were collected.

## Method and provenance

Each mode compares current Relar, the preserved f-string Relar executable, CPython 3.14.5, and Rust BigInt on six captured inputs. CPython and current Relar use the migrated keyword-call source; preserved Relar uses the previous positional-call source. The explicit baseline snapshot verifies the complete old six-file package against its original record and captures exactly five changed files. Historical CPython ratios cannot isolate compiler effects from this source change or host variation.

Rust references were rebuilt from the measured commit because their Cargo manifest gained a license field. Algorithm/dependency sources, tools, flags, and linking budgets match the previous reference, and both rebuilt executables are byte-identical to it. Final compiler/runtime artifacts match those used for validation; the runtime archives are unchanged from the preceding milestone.

Each workload has 48 measured rounds covering all 24 engine permutations twice, plus two warmup rounds. Measured positions and directed within-round predecessors are balanced; warmups, cross-round predecessors, and workload order are not. Records retain **2,304 measured executions, 96 warmups, and 48 workload gates**. Every execution checks status, stdout/stderr, report bytes, and filesystem contents. Each mode also checks all 176 independent oracle cases and `/dev/full` across its four engines: **1,408 oracle outcomes and 8 device outcomes**.

Fresh processes are pinned to CPU 0 outside ptrace. Timing includes startup/imports, file reading/decoding/parsing, eager containers and records, reconciliation, complete rendering, buffered writes, flush/close, and exit. Fixture preparation, builds, oracles, hashing, comparisons, and deletion are outside timing. OS caches are not deliberately flushed and writes are not fsynced. CPython's normal bytecode cache is prepared outside timing and verified unchanged.

Modes ran sequentially after task-owned builds, tests, and profiling stopped. CPU frequency and unrelated shared-host activity remain uncontrolled. This is one x86-64 AMD EPYC-Milan devbox. CPython is a GIL build configured with PGO/LTO and its experimental JIT disabled. Relar uses Clang 18.1.3 at `-O3`; ThinLTO uses LLD 22.1.8. Rust 1.98.1-dev builds disable Ohm's experimental defaults; ThinLTO LLVM budgets match at 1,000. Rust preserves the eager algorithm and arbitrary-precision quantities, using native containers, borrowed strings, and direct formatting.

## Validation and retained evidence

All **2,376 reported semantic-suite checks**, **66 Rust unit tests**, format/Clippy checks, and 19 pure-Python benchmark-guard checks pass. Semantic totals include compile-time rejections. Coverage includes ordinary `-O0`/`-O3`, ThinLTO `-O3`, sanitizer `-O1`, and selected sanitizer `-O0` suites. Separate integration checks compare 176 inventory cases across nine old/new-source and engine combinations. Instrumented Relar also matches CPython and Rust on those 176 cases plus `/dev/full`. Sanitizers cover generated code and runtime dependencies; the prebuilt Rust standard library is not instrumented.

Initial sandbox sanitizer runs failed because LeakSanitizer could not use ptrace. The twelve affected general suites and the inventory application passed outside the sandbox with unchanged compiler/runtime/source inputs and leak detection enabled. Original failure logs and receipts, the inventory's partial initial receipt, and successful retries are retained. The [validation index](2026-10-19-keyword-arguments-validation.json) distinguishes captured before/after hashes from later source reconciliation and records provenance limits.

The [timing audit](2026-10-19-keyword-arguments-audit.json) verifies 28,002 assertions and 109 distinct file hashes. [Ordinary](2026-10-19-keyword-arguments-normal.json.gz) and [ThinLTO](2026-10-19-keyword-arguments-lto.json.gz) raw records preserve exact original JSON bytes under deterministic gzip. The [build](2026-10-19-keyword-arguments-build.json) and [session](2026-10-19-keyword-arguments-session.json) records retain commands, versions, hashes, and source identities.

The [evidence archive](2026-10-19-keyword-arguments-evidence.tar.gz) contains the measured source snapshot, generated LLVM, preserved old package, build/validation records and logs, failures and retries, and launcher/audit/packaging scripts. Its [manifest](2026-10-19-keyword-arguments-evidence-manifest.json) hashes every member and companion. Native binaries, runtime archives, interpreters, toolchains, bytecode caches, and historical results are excluded. Their original paths/hashes and published baseline references remain; audits that inspect these external artifacts still require them. The published samples, sources, and logs support independent statistical and evidence review.
