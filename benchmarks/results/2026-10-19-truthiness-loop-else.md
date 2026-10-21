> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Truthiness, boolean values, and loop else

Relar now supports Python truth tests, operand-returning `and`/`or`, and `for`/`while` `else` suites. The inventory reconciler uses all three capabilities while preserving its input, error, and output behavior. On the two large workloads, ThinLTO Relar runs **3.61× faster than CPython for dense input and 2.75× for sparse input**, taking **4.1% and 8.7% more time than Rust BigInt**, respectively.

[Truthiness and boolean values](https://github.com/viarius-experiments/relar/pull/42) and [loop else](https://github.com/viarius-experiments/relar/pull/43) continue GitHub Stack 4 after the [floats, bytes, matching, and assignment-expression milestone](2026-10-19-floats-bytes-match-walrus.md). Measurements use clean source commit `5b3e8438376091340f6610d7e603b9808d7ed65c`; the results and README refresh are committed separately.

## Capability and application

- **Truthiness:** conditions, assertions, comprehension filters, match guards, `not`, and `bool(value)` use Python's truth rules for supported concrete types. Numeric zero, `None`, and empty containers are false; NaN is true and both floating-point signed zeros are false. Class instances, exceptions, and file handles are true. `bool()` returns `False`. Scalar tests lower directly to LLVM, and string/bytes truth checks do not scan their contents.
- **Boolean values:** `and` and `or` evaluate left to right, short-circuit, and return the selected operand while preserving shared identity and ownership. Stored results must fit a concrete type or its `T | None` form; truth contexts can directly consume heterogeneous operands. A true optional local narrows to its payload; a false optional value can still be a present zero or empty container. Type annotations remain enforced.
- **Loop else:** exhaustion, including zero iterations, runs the `else` suite. `break`, `return`, and escaping exceptions skip it. Nested loops, `continue`, `finally`, definite assignment, optional narrowing, and compiler-owned iterator cleanup preserve the corresponding Python behavior.

The inventory parser uses `not token` and a digit-scanning `for ... else`; the stock reader uses a countdown `while ... else`. Reconciliation uses one dictionary `get` and an operand-returning `and`/`or` expression for the filled quantity, retaining an explicit `is not None` check when missing and zero stock must differ. The [language contract](../../docs/subset.md) documents the exact boundaries, including unsupported general unions and user-defined `__bool__`/`__len__` protocols.

## Updated-application comparisons

CPython and current Relar run the updated application. Preserved Relar runs the previous application because the previous compiler cannot accept the new syntax. Rust uses its unchanged source and executables. These comparisons therefore include both compiler/runtime changes and the application rewrite. Times are median milliseconds; positive change means more time than preserved Relar.

**Ordinary linking**

| Input | CPython ms | Previous Relar ms | Current Relar ms | Rust BigInt ms | Relar time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.136 | 2.148 | 2.137 | 2.151 | -0.54% |
| boundary-128 | 28.555 | 2.893 | 2.866 | 2.614 | -0.91% |
| dense-2048 | 33.673 | 6.430 | 6.247 | 5.908 | -2.85% |
| sparse-2048 | 37.949 | 9.290 | 9.041 | 8.423 | -2.68% |
| dense-16384 | 77.041 | 27.435 | 26.147 | 23.167 | -4.70% |
| sparse-16384 | 111.511 | 50.858 | 49.611 | 42.592 | -2.45% |

**ThinLTO**

| Input | CPython ms | Previous Relar ms | Current Relar ms | Rust BigInt ms | Relar time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.481 | 2.135 | 2.133 | 2.168 | -0.08% |
| boundary-128 | 28.611 | 2.832 | 2.805 | 2.667 | -0.97% |
| dense-2048 | 33.887 | 5.735 | 5.634 | 5.597 | -1.76% |
| sparse-2048 | 37.536 | 8.011 | 7.933 | 7.730 | -0.98% |
| dense-16384 | 76.696 | 21.641 | 21.237 | 20.398 | -1.87% |
| sparse-16384 | 112.375 | 41.463 | 40.878 | 37.599 | -1.41% |

For 16,384 positive requests, ordinary Relar takes 2.5–4.7% less time than preserved Relar and runs 2.25–2.95× faster than CPython. ThinLTO takes 1.4–1.9% less time than preserved Relar and narrows the Rust gap to 4.1–8.7%. These are descriptive changes in this session, not a general compiler speedup claim. Small cases include substantial process-startup and small-file overhead. No new CPU or allocation profiles were collected.

## Unchanged-application controls

Both current and preserved Relar also run the exact previous application, holding all six Python files fixed. CPython and Rust participate in the same balanced rounds. This isolates the compiler/runtime comparison from adopting the new syntax.

| Input | Ordinary previous ms | Ordinary current ms | Change | ThinLTO previous ms | ThinLTO current ms | Change |
|---|---:|---:|---:|---:|---:|---:|
| empty | 2.104 | 2.103 | -0.02% | 2.125 | 2.099 | -1.23% |
| boundary-128 | 2.911 | 2.913 | +0.06% | 2.719 | 2.679 | -1.48% |
| dense-2048 | 6.691 | 6.639 | -0.78% | 5.736 | 5.676 | -1.04% |
| sparse-2048 | 9.395 | 9.378 | -0.18% | 8.007 | 7.939 | -0.86% |
| dense-16384 | 27.279 | 27.002 | -1.02% | 21.655 | 21.522 | -0.62% |
| sparse-16384 | 50.844 | 50.647 | -0.39% | 41.436 | 41.265 | -0.41% |

Across these twelve comparisons, median changes range from **-1.48% to +0.06%**, and every current/preserved Relar interquartile range overlaps. This provides no clear compiler-only regression signal. Interquartile ranges describe sample spread, not confidence intervals or proof of equivalent performance. The application and unchanged-source families ran sequentially; subtracting their changes would not establish a causal source-only effect. The [summary](2026-10-19-truthiness-loop-else-summary.json) retains all four engines' statistics in every family.

## Method and provenance

The session ran on October 19, 2026, from 11:25:19 to 11:28:41 UTC. Each of four families uses six workloads, 48 measured rounds covering all 24 engine permutations twice, and two warmup rounds. Measured execution positions and directed within-round predecessors are balanced; warmups, cross-round predecessors, and workload order are not. Records retain **4,608 measured executions, 192 warmups, and 96 workload gates**.

Before timing, every family checks the independent 204-case oracle and `/dev/full` on all four engines: **3,264 oracle outcomes and 16 device-error outcomes**. Every timed run and warmup checks exit status, stdout/stderr, complete report bytes, and filesystem effects. The launcher verifies clean source, frozen tools, preserved references, and unchanged CPython bytecode caches before and after the session.

Fresh processes run on CPU 0 outside ptrace. Timing includes startup/imports, reading/decoding/parsing, reconciliation, rendering, buffered writes, flush/close, and exit. Builds, fixture preparation, oracles, hashing, verification, and deletion stay outside the clock. OS caches are warm; writes are not fsynced. Task-owned builds, tests, and profiling stop during measurements; CPU frequency and unrelated activity on the shared host remain uncontrolled.

The host is x86-64 AMD EPYC-Milan. CPython 3.14.5 is a GIL build with its experimental JIT disabled and PGO/LTO enabled. Relar uses Clang 18.1.3 at `-O3`, with LLD 22.1.8 for cross-language ThinLTO and matched LLVM optimization budgets. Rust preserves the eager algorithm and arbitrary-precision quantities while using native containers, borrowed strings, and direct formatting. These results apply to this complete inventory program and recorded environment.

## Validation and evidence

All **1,654 semantic/regression checks** pass: **1,202 ordinary-linking checks** across all 17 suites, **187 targeted ThinLTO checks**, and **265 targeted ASan/LSan checks**. Counts include compile-time rejections. Targeted suites cover truthiness, loop else, assignment expressions, exceptions, and files; ordinary checks also cover existing imports, classes, containers, comprehensions, calls, formatting, matching, scalar types, and sequence access. The compiled source passes **86 Rust unit tests**, Clippy with warnings denied, formatting, and focused Ruff checks.

The updated inventory application separately passes **204 oracle cases and `/dev/full`** under O1 ASan/LSan outside ptrace, with leak detection enabled. Sanitizers instrument generated code and runtime dependencies; the prebuilt Rust standard library remains uninstrumented.

One old matching rejection still expected optional-bool guards to fail. It was replaced with a positive regression and the matching suite was rerun successfully. The original failure and retry are retained. Rebuilding after this test-only restack produced byte-identical compiler and ordinary/ThinLTO/ASan runtime artifacts; all 35 compiled source inputs match the earlier validation build. The final [build record](2026-10-19-truthiness-loop-else-build.json), [validation index](2026-10-19-truthiness-loop-else-validation.json), and archived restack proof connect both runs to the measured source.

The [independent audit](2026-10-19-truthiness-loop-else-audit.json) passed **358,308 checks** of raw integer-nanosecond samples, exact quantiles, execution order, outcomes, source-family separation, and provenance. The [session record](2026-10-19-truthiness-loop-else-session.json) retains commands and actual timestamps. Compressed original records are available for [application/ordinary](2026-10-19-truthiness-loop-else-application-normal.json.gz), [application/ThinLTO](2026-10-19-truthiness-loop-else-application-lto.json.gz), [unchanged-source/ordinary](2026-10-19-truthiness-loop-else-common-source-normal.json.gz), and [unchanged-source/ThinLTO](2026-10-19-truthiness-loop-else-common-source-lto.json.gz).

The [evidence archive](2026-10-19-truthiness-loop-else-evidence.tar.gz) and [manifest](2026-10-19-truthiness-loop-else-evidence-manifest.json) retain source, generated LLVM, validation logs, raw records, and reproduction/audit scripts. Native executables, runtime archives, interpreters, toolchains, and bytecode caches are excluded. A portable publication audit verifies retained text/source evidence; rerunning the complete live artifact audit requires the original native artifacts identified in the records.
