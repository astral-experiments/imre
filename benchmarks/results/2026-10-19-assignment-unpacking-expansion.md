> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Chained assignment, sequence unpacking, and container expansion

Relar now supports chained assignment, nested and starred sequence unpacking, and typed list/set/dictionary expansion. The inventory reconciler uses all three while preserving its input, error, and report behavior. On the two large workloads, ThinLTO Relar runs **3.63× faster than CPython for dense input and 2.71× for sparse input**. Its median time relative to Rust BigInt is **+1.9% and +9.7%**, respectively; a positive value means Relar takes more time.

The [compiler implementation](https://github.com/viarius-experiments/relar/pull/45) continues GitHub Stack 4 after the [truthiness and loop-else milestone](2026-10-19-truthiness-loop-else.md). Measurements and final validation use clean source commit `3cc767b401f988ece94e1eb4bb98c43cd06c02b2`. Results and the README refresh are committed afterward.

## Capability and application

- **Chained assignment:** the RHS evaluates once; targets store from left to right and retain shared mutable identity. Declared targets can provide context for empty literals.
- **Sequence unpacking:** fixed tuples, lists, strings, and bytes support nested patterns and one starred target per level. Targets include names, fields, and list/dictionary subscripts, including in container loops and comprehensions. Each level snapshots its values before storing targets; later nested failures preserve earlier stores. Dynamic length failures raise `ValueError`. Starred remainders are fresh shallow lists with concrete element layouts.
- **Container expansion:** list/set `*source` uses existing typed iteration sources, including dictionary keys, strings, bytes, range/enumerate, and compatible fixed tuples. Dictionary `**source` copies concrete mappings in order. Each expansion completes before the next entry evaluates; duplicate keys retain their original position while the last value wins.

Exact storage types remain enforced. Fixed-tuple arity errors stay static, and tuple result expansion, call argument expansion, variadic parameters, and user-defined iterator protocols remain outside this milestone. The [language contract](../../docs/subset.md#assignment-and-container-expansion) details context, aliases, constructors, and remaining boundaries.

The inventory entry point unpacks `sys.argv` with a starred remainder and checks the three paths through sequence unpacking. The parser unpacks count and SKU/quantity rows while retaining the previous diagnostics and conversion order. Reconciliation expands requested dictionary keys into a membership set; rendering initializes three totals with chained assignment. Rust retains its existing eager algorithm, arbitrary-precision quantities, source, and executables.

The compiler reuses existing typed primitives and scoped IR. Tuple scalarization and integer range analysis now account for expression-scope writes and cleanup, including short-circuit joins and while conditions. These changes preserve the existing concrete storage and native index optimizations. Runtime sources and the private ABI are unchanged.

## Updated-application comparisons

CPython and current Relar run the updated application. Preserved Relar runs the previous application because its compiler cannot accept the new syntax. These comparisons combine the compiler change and application rewrite. Times are median milliseconds; positive time change means more time than preserved Relar.

**Ordinary linking**

| Input | CPython ms | Previous Relar ms | Current Relar ms | Rust BigInt ms | Relar time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.007 | 2.124 | 2.129 | 2.143 | +0.25% |
| boundary-128 | 27.618 | 2.731 | 2.739 | 2.542 | +0.31% |
| dense-2048 | 33.082 | 6.218 | 6.313 | 5.850 | +1.53% |
| sparse-2048 | 38.245 | 9.078 | 9.441 | 8.475 | +4.00% |
| dense-16384 | 77.661 | 26.434 | 26.694 | 23.337 | +0.98% |
| sparse-16384 | 112.346 | 50.247 | 52.241 | 43.051 | +3.97% |

**ThinLTO**

| Input | CPython ms | Previous Relar ms | Current Relar ms | Rust BigInt ms | Relar time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.140 | 2.207 | 2.158 | 2.243 | -2.22% |
| boundary-128 | 27.938 | 2.770 | 2.712 | 2.591 | -2.09% |
| dense-2048 | 33.534 | 5.702 | 5.626 | 5.572 | -1.34% |
| sparse-2048 | 37.774 | 8.018 | 8.026 | 7.843 | +0.09% |
| dense-16384 | 76.639 | 21.372 | 21.090 | 20.699 | -1.32% |
| sparse-16384 | 112.605 | 40.841 | 41.584 | 37.917 | +1.82% |

The large ordinary-linking medians take +0.98% and +3.97% more time than preserved Relar. The corresponding ThinLTO changes are -1.32% and +1.82%. Both ordinary sparse workloads have nonoverlapping current/previous interquartile ranges; all six ThinLTO application pairs overlap. This is a compatibility milestone with a modest ordinary-linking regression, not a native speedup claim.

These are complete-program measurements, not isolated unpacking costs. Small cases include substantial process startup and small-file overhead. No new CPU or allocation profiles were collected.

## Unchanged-application controls

Current and preserved Relar also run the exact previous six-file application, with CPython and unchanged Rust participating in the same balanced rounds. This holds application source fixed for the compiler comparison.

| Input | Ordinary previous ms | Ordinary current ms | Change | ThinLTO previous ms | ThinLTO current ms | Change |
|---|---:|---:|---:|---:|---:|---:|
| empty | 2.116 | 2.125 | +0.41% | 2.409 | 2.337 | -2.98% |
| boundary-128 | 2.749 | 2.781 | +1.17% | 2.975 | 2.989 | +0.46% |
| dense-2048 | 6.223 | 6.350 | +2.04% | 5.814 | 5.786 | -0.48% |
| sparse-2048 | 9.031 | 9.249 | +2.41% | 7.915 | 7.956 | +0.51% |
| dense-16384 | 26.393 | 26.857 | +1.76% | 21.607 | 21.857 | +1.16% |
| sparse-16384 | 49.727 | 50.852 | +2.26% | 41.519 | 41.808 | +0.69% |

Across these twelve comparisons, median changes range from **-2.98% to +2.41%**. 3 of the twelve current/preserved Relar interquartile-range pairs do not overlap. Interquartile ranges describe sample spread; they are not confidence intervals or proof of equivalent performance. Application and unchanged-source families ran as separate sequential batches, so subtracting their changes would not establish a causal source-only effect. The [summary](2026-10-19-assignment-unpacking-expansion-summary.json) retains all four engines' statistics and exact quantiles in every family.

## Method and provenance

The complete session ran on October 19, 2026, from 13:48:53 to 13:52:16 UTC. Four families each use six workloads, 48 measured rounds covering all 24 engine permutations twice, and two warmup rounds: **4,608 measured executions, 192 warmups, and 96 workload gates**. Measured execution positions and directed within-round predecessors are balanced. Warmups, cross-round predecessors, and workload order are not balanced.

Before timing, every family checks the independent 204-case oracle and `/dev/full` on all four engines: **3,264 oracle outcomes and 16 device-error outcomes**. Every timed run and warmup checks exit status, stdout/stderr, complete report bytes, and filesystem effects. Clean source, frozen tools, preserved references, and CPython bytecode caches are verified before and after the session.

Fresh processes run on CPU 0 outside the execution sandbox. Timing includes startup/imports, file reading/decoding/parsing, reconciliation, rendering, buffered writes, flush/close, and exit. Builds, fixture preparation, oracles, hashing, verification, and deletion stay outside the clock. OS caches are warm; writes are not fsynced. All task-owned builds, tests, and profiling stop during timing. CPU frequency and unrelated activity on the shared host remain uncontrolled.

CPython is 3.14.5. Relar uses Clang 18 at `-O3`, with the pinned Rust toolchain's LLD for cross-language ThinLTO and matched LLVM optimization budgets. Rust uses native containers, borrowed strings, direct formatting, and arbitrary-precision integers. The raw records retain exact tool versions, configurations, source hashes, and environment details. These results apply to this program and recorded environment.

## Validation and evidence

All **2,417 semantic/regression checks** pass: **1,279 ordinary checks** across all 18 suites, **490 targeted ThinLTO checks**, and **648 targeted ASan/LSan checks**. Counts include static rejections. Targeted suites cover unpacking/expansion, assignment expressions, comprehensions, containers, classes, exceptions, and files. The source also passes **92 Rust unit tests**, Clippy with warnings denied, formatting, and focused Ruff checks.

The inventory separately passes **204 oracle cases and `/dev/full`** under O1 ASan/LSan, with leak detection enabled and no diagnostics. Initial sandboxed sanitizer attempts aborted during LeakSanitizer process inspection; unchanged inputs passed with sandbox permission, and both failed-attempt logs and successful receipts are retained. Sanitizers cover generated application code and the custom runtime; the prebuilt Rust standard library remains uninstrumented.

Relar's existing exception model has no traceback/frame references. Exited frames release their owners during propagation, so implicit file closure can precede CPython's release of traceback-held locals. This preexisting boundary is documented in the subset guide; explicit `with` and `close` provide deterministic file handling.

The [independent audit](2026-10-19-assignment-unpacking-expansion-audit.json) passed **360,225 checks** over raw integer-nanosecond samples, exact quantiles, order, outcomes, source-family separation, and provenance. The [build record](2026-10-19-assignment-unpacking-expansion-build.json), [session](2026-10-19-assignment-unpacking-expansion-session.json), and [validation index](2026-10-19-assignment-unpacking-expansion-validation.json) connect the final results to the measured source and frozen artifacts.

Original records are available for [application/ordinary](2026-10-19-assignment-unpacking-expansion-application-normal.json.gz), [application/ThinLTO](2026-10-19-assignment-unpacking-expansion-application-lto.json.gz), [unchanged-source/ordinary](2026-10-19-assignment-unpacking-expansion-common-source-normal.json.gz), and [unchanged-source/ThinLTO](2026-10-19-assignment-unpacking-expansion-common-source-lto.json.gz). The [evidence archive](2026-10-19-assignment-unpacking-expansion-evidence.tar.gz) and [manifest](2026-10-19-assignment-unpacking-expansion-evidence-manifest.json) retain source, generated LLVM, logs, raw records, and reproduction/audit scripts. Native executables, runtime archives, interpreters, toolchains, and bytecode caches remain external and are identified by hashes. The [portable audit](2026-10-19-assignment-unpacking-expansion-portable-audit.json) verifies the retained text/source evidence without executing native artifacts.
