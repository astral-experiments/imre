> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Conditional expressions and sequence access: inventory reconciliation

The inventory reconciler now uses conditional expressions, string indexing and iteration, and list/string slicing while preserving its input, report, error, and filesystem behavior. On the two large workloads, ThinLTO Imre runs **2.68–3.60× faster than CPython** and takes **5.0–10.5% more time than Rust BigInt**. This is a capability milestone; the measurements do not establish a native speedup.

[PR 35](https://github.com/viarius-experiments/imre/pull/35) continues GitHub Stack 4 after the [keyword-argument milestone](2026-10-19-keyword-arguments.md). All measurements use clean source commit `006e24367b87ecc331f31605640a7e53d3bdeaa6`. The report and documentation are committed separately. The filename uses the client-local date, October 18; records retain the actual October 19 UTC timestamps.

## Supported syntax and example changes

A conditional expression evaluates its explicit boolean condition once and only evaluates the selected arm. Both arms are type-checked; contextual types and local optional narrowing apply. String indexing accepts exact integers, supports negative indexes, and raises `IndexError` outside the string. Indexing, slicing, and iteration use Unicode scalar values. String iteration advances linearly without repeatedly scanning from the start.

Slicing supports concrete lists and strings, omitted/`None`/optional-integer components, arbitrary-precision bounds, and positive or negative steps. Bounds clip to the sequence, zero steps raise `ValueError`, and list slices produce independent shallow containers. Receiver and bound expressions retain source evaluation order and ownership across mutation or failure. Tuple slicing, string assignment, and slice assignment/deletion remain outside the [language contract](../../docs/subset.md#conditional-expressions-and-sequence-access).

Three inventory modules change. CLI handling uses `arguments = sys.argv[1:]`; fulfillment selects `available if available < quantity else quantity`. A bounded frame-count scanner reads the optional sign with `token[0]`, removes it with `token[1:]`, and iterates the remaining characters, allowing underscores only between digits. It delegates individual digit conversion to the existing integer parser.

The scanner only handles nonempty tokens of at most 18 characters: all intermediate decimal values fit the native small-integer representation, and the token is below CPython's minimum configurable decimal conversion limit. Longer tokens retain the previous `int()` path and its configured digit-limit behavior. Quantity parsing, framing, negative-count checks, and error labels/order remain unchanged. CPython continues to accept Unicode decimal digits where its `int()` did before; Imre's existing integer parser remains ASCII-only, and the shared benchmark input domain remains ASCII decimal integers. Dedicated old/new-source comparisons cover Unicode digits as well as signs, underscores, whitespace, huge counts, and digit-limit boundaries.

## Primary comparisons

Times are median milliseconds. Positive change means more time than the preserved keyword-argument executable. CPython and current Imre run the new source; preserved Imre runs the old source. Consequently, these comparisons include both application-source and compiler/runtime changes.

Across all twelve primary comparisons, Imre median time changes range from **−1.34% to +1.63%**. Every previous/current interquartile range overlaps. The [summary](2026-10-18-sequence-access-summary.json) retains exact medians, Q1/Q3, ratios, and overlap checks. Interquartile ranges describe sample spread, not confidence intervals or proof of equivalent performance.

**Ordinary**

| Input | CPython ms | Previous Imre ms | Current Imre ms | Rust BigInt ms | Imre time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.409 | 2.230 | 2.213 | 2.216 | -0.76% |
| boundary-128 | 28.732 | 2.989 | 2.972 | 2.800 | -0.58% |
| dense-2048 | 34.321 | 6.644 | 6.589 | 6.206 | -0.83% |
| sparse-2048 | 38.025 | 9.362 | 9.236 | 8.534 | -1.34% |
| dense-16384 | 77.086 | 26.817 | 26.652 | 23.008 | -0.61% |
| sparse-16384 | 112.997 | 51.335 | 51.070 | 43.279 | -0.52% |

**ThinLTO**

| Input | CPython ms | Previous Imre ms | Current Imre ms | Rust BigInt ms | Imre time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.175 | 2.116 | 2.150 | 2.154 | +1.63% |
| boundary-128 | 27.657 | 2.660 | 2.676 | 2.517 | +0.57% |
| dense-2048 | 33.395 | 5.653 | 5.657 | 5.551 | +0.08% |
| sparse-2048 | 37.192 | 7.882 | 7.920 | 7.634 | +0.49% |
| dense-16384 | 77.320 | 21.419 | 21.476 | 20.451 | +0.26% |
| sparse-16384 | 112.009 | 41.345 | 41.756 | 37.785 | +1.00% |

For the 16,384-positive-request cases, ordinary Imre runs 2.21–2.89× faster than CPython and takes 15.8–18.0% more time than Rust BigInt. ThinLTO narrows that Rust gap to 5.0–10.5%. Small cases include a substantial startup and small-file floor. No new CPU or allocation profiles were collected.

## Unchanged-source native controls

A separate comparison recompiles the exact previous six-file inventory package with the new frozen compiler/runtime and compares it with the preserved previous executable. It uses the same captured inputs, matching ordinary/ThinLTO flags, complete oracle checks, and 48 alternating measured pairs per workload. These controls measure compiler/runtime/build effects while holding application source fixed.

| Mode | Input | Previous tools ms | Current tools ms | Time change |
|---|---|---:|---:|---:|
| Ordinary | empty | 2.115 | 2.199 | +3.97% |
| Ordinary | boundary-128 | 2.744 | 2.753 | +0.33% |
| Ordinary | dense-2048 | 6.249 | 6.309 | +0.96% |
| Ordinary | sparse-2048 | 8.961 | 9.004 | +0.48% |
| Ordinary | dense-16384 | 26.488 | 26.687 | +0.75% |
| Ordinary | sparse-16384 | 50.670 | 50.767 | +0.19% |
| ThinLTO | empty | 2.086 | 2.088 | +0.06% |
| ThinLTO | boundary-128 | 2.673 | 2.669 | -0.18% |
| ThinLTO | dense-2048 | 5.619 | 5.590 | -0.51% |
| ThinLTO | sparse-2048 | 8.073 | 8.035 | -0.47% |
| ThinLTO | dense-16384 | 21.139 | 20.997 | -0.67% |
| ThinLTO | sparse-16384 | 40.988 | 40.520 | -1.14% |

All twelve control interquartile ranges overlap. The two large ordinary controls change by +0.75%/+0.19%; the large ThinLTO controls change by −0.67%/−1.14%. Generated LLVM differs from the preceding milestone in both primary and unchanged-source builds. These are separate timing sessions, so subtracting control percentages from primary percentages would not isolate a precise source-change cost.

## Method and provenance

The main comparisons use the same six captured workloads as the preceding milestone, 48 measured rounds covering all 24 engine permutations twice, and two warmup rounds. Measured execution positions and directed within-round predecessors are balanced; warmups, cross-round predecessors, and workload order are not. The preserved old package matches its original record exactly, and exactly `__main__.py`, `load.py`, and `reconcile.py` differ in the new package. The Rust source, build records, and native executables are reused unchanged after hash verification.

Primary records retain **2,304 measured executions, 96 warmups, and 48 workload gates**. Native controls add **1,152 measured executions, 48 warmups, and 24 gates**, with both pair orders appearing 24 times. Every run checks status, stdout/stderr, report bytes, and complete filesystem effects. Before timing, each configuration checks all 204 independent inventory oracle cases plus `/dev/full`: **2,448 oracle outcomes and 12 device outcomes** across the primary/control groups.

Fresh processes run on CPU 0 outside ptrace. Timing includes startup/imports, file reading/decoding/parsing, eager containers and records, reconciliation, rendering, buffered writes, flush/close, and exit. Fixture preparation, builds, oracles, hashing, verification, and deletion remain outside the clock. OS caches are warm; writes are not fsynced. CPython's normal bytecode cache is prepared outside timing and verified unchanged.

Task-owned builds, tests, and profiling stopped before measurements. Modes and controls run sequentially; CPU frequency and unrelated activity on the shared host remain uncontrolled. The host is x86-64 AMD EPYC-Milan. CPython 3.14.5 is a GIL build configured with PGO/LTO, with its experimental JIT disabled. Imre uses Clang 18.1.3 at `-O3`, and ThinLTO uses LLD 22.1.8 with matched LLVM optimization budgets. The Rust BigInt reference preserves the eager algorithm and arbitrary-precision quantities, using native containers, borrowed strings, and direct formatting. These are whole-program results for this inventory example, not a general Python performance claim.

## Validation and retained evidence

All **2,246 native semantic/regression checks** pass: 884 ordinary, 673 ThinLTO, and 689 ASan/LSan. Totals include compile-time rejections. The new suite covers lazy arms, contextual/optional types, Unicode access and iteration, shallow aliases, mutation and failure ordering, and 1,701 slice checks apiece for strings and integer lists. Existing calls, classes, containers, comprehensions, imports, exceptions, formatting, I/O, and file suites pass too. A validation receipt also records 74 passing Rust unit tests, formatting, Clippy, and Ruff checks on the changed Python files.

The final inventory application separately passes all **204 oracle cases and `/dev/full` with ASan and leak detection enabled**, after timings finish and outside ptrace. Earlier source-migration checks compare current CPython, previous CPython, preserved Imre, and Rust on the same 204 cases. A dedicated CPython parser differential verifies **32,505 old/new outcomes** at decimal digit limits 640, 4,300, and disabled. The added oracle cases cover the scanner's 18/19-character boundary and preserve malformed-input error precedence.

Eleven initial sanitizer suite attempts failed because LeakSanitizer cannot operate under ptrace. Successful retries outside the sandbox supersede them; original failure logs and available receipts remain in the evidence archive. Frozen compiler/runtime hashes match the tested artifacts and before/after regression manifests. Sanitizers instrument generated code and runtime dependencies; the prebuilt Rust standard library remains uninstrumented. The [validation index](2026-10-18-sequence-access-validation.json) records the joins and provenance limits.

The [timing audit](2026-10-18-sequence-access-audit.json) verifies **45,868 assertions and 114 distinct file hashes**, independently recomputing every timing summary from raw integer nanoseconds. [Ordinary](2026-10-18-sequence-access-normal.json.gz) and [ThinLTO](2026-10-18-sequence-access-lto.json.gz) primary records and the [ordinary](2026-10-18-sequence-access-normal-source-control.json.gz)/[ThinLTO](2026-10-18-sequence-access-lto-source-control.json.gz) controls retain exact original JSON bytes under deterministic gzip. [Build](2026-10-18-sequence-access-build.json) and [session](2026-10-18-sequence-access-session.json) records preserve commands, tool versions, source identities, hashes, and actual measurement timestamps.

The [evidence archive](2026-10-18-sequence-access-evidence.tar.gz) contains the measured source tree, old package, generated LLVM, raw comparisons, build and validation records/logs, and launcher/audit/packaging scripts. Its [manifest](2026-10-18-sequence-access-evidence-manifest.json) hashes every member and companion. Native binaries, runtime archives, interpreters, toolchains, bytecode caches, and historical result files are excluded. Original paths/hashes and published baseline references remain, so the full timing audit still requires those artifacts. The included publication audit independently checks archived bytes and recomputes all saved timing and oracle results using only the published text/source.
