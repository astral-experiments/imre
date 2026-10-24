> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Named incremental planner: iteration, dictionaries, and sets

Imre runs the four large named incremental-planner workloads **2.28–4.67× faster than CPython 3.14.5** with matched ThinLTO builds. It takes **1.49–1.57× as long as Rust BigInt**, or **1.76–1.85× as long as checked-i64 Rust**. These are median whole-process times on one recorded host, including input, parsing, validation of the entire graph, affected-target discovery, scheduling, the full report, and exit.

This is a new program with different work from the earlier dependency-planner benchmark. Its timings do not measure a regression or speedup in that earlier program. The [Python package](../../examples/incremental_planner/README.md) runs unchanged on CPython and Imre. The [Rust reference](../reference/incremental_planner/README.md) directly ports the same validation phases, dependency deduplication, heaps, affected-set closure, and reporting algorithm.

**Functionality.** The stack first adds direct list iteration, `enumerate`, and fixed tuple unpacking; then insertion-ordered `dict[int, V]` and `dict[str, V]` with lookup, assignment, deletion, membership, `get`, key iteration, and `.items()`; then `set[int]` and `set[str]` with literals, contextual empty construction, membership, `add`, `discard`, and iteration. Iterators retain their source and own yielded values across the body. List iteration observes the current length; dictionary/set size changes are checked before the next step. Set order and same-size structural mutation traversal are unspecified. These are concrete built-in operations; [the subset guide](../../docs/subset.md) documents their boundaries. The runtime continues to use reference counting and reject cyclic heap type graphs.

The real example resolves target names, deduplicates repeated dependencies and changes, validates even unaffected components, and schedules the transitively affected targets. Ready targets use the lowest input ID, so output is deterministic. Unaffected prerequisites are treated as already built. Error precedence is duplicate names, negative durations, unknown dependencies, unknown changed names, then cycles. Durations and schedule times retain arbitrary-precision semantics.

**Matched ThinLTO results.** All times are milliseconds; lower is better. Each implementation has 32 fresh-process samples per workload, after two warmups.

| Workload | Affected | CPython ms | Imre ms | Rust BigInt ms | Rust i64 ms | CPython / Imre | Imre / Rust BigInt |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| layered-16384-sparse | 427 | 114.150 | 50.167 | 32.039 | 28.548 | 2.28× | 1.57× |
| layered-16384-all | 16,384 | 193.074 | 73.260 | 49.027 | 41.075 | 2.64× | 1.49× |
| wide-16384-sparse | 1 | 102.584 | 23.313 | 15.234 | 12.608 | 4.40× | 1.53× |
| wide-16384-all | 16,384 | 189.457 | 40.570 | 26.516 | 22.496 | 4.67× | 1.53× |
| boundary-128-all | 128 | 20.848 | 3.578 | 3.457 | 3.204 | 5.83× | 1.03× |

The layered graph has 16,384 targets and 38,901 unique dependencies. A single changed target affects 427 targets. The wide graph has 16,384 independent targets; its sparse case changes and schedules one target, while still parsing and validating every target. Full-change cases schedule all targets. The 128-target boundary case uses multibyte names and durations around tagged-integer and signed-64-bit limits; it remains within all four implementations' numeric domains. Its short duration gives startup and I/O overhead more influence than in the large cases.

Observed interquartile ranges for the matched ThinLTO runs are below. They describe sample spread, not confidence intervals. The raw records retain minima, maxima, and quartiles for every engine.

| Workload | Imre p25–p75 ms | Rust BigInt p25–p75 ms |
| --- | ---: | ---: |
| layered-16384-sparse | 49.679–50.714 | 31.847–32.398 |
| layered-16384-all | 72.360–74.014 | 48.458–49.513 |
| wide-16384-sparse | 23.036–23.504 | 15.087–15.309 |
| wide-16384-all | 39.911–41.109 | 26.169–27.212 |
| boundary-128-all | 3.511–3.620 | 3.429–3.506 |

**Ordinary-link controls.** Imre and both Rust references use release O3 without explicit cross-language ThinLTO or LLVM budget overrides. The same inputs and ordering method are used in a separate run.

| Workload | Affected | CPython ms | Imre ms | Rust BigInt ms | Rust i64 ms | CPython / Imre | Imre / Rust BigInt |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| layered-16384-sparse | 427 | 114.358 | 57.158 | 34.989 | 31.041 | 2.00× | 1.63× |
| layered-16384-all | 16,384 | 192.381 | 83.705 | 53.292 | 45.080 | 2.30× | 1.57× |
| wide-16384-sparse | 1 | 103.036 | 28.570 | 16.882 | 13.213 | 3.61× | 1.69× |
| wide-16384-all | 16,384 | 187.838 | 50.629 | 30.101 | 24.695 | 3.71× | 1.68× |
| boundary-128-all | 128 | 20.908 | 3.734 | 3.547 | 3.384 | 5.60× | 1.05× |

**Method and interpretation.** The host is Linux x86-64 on an AMD EPYC-Milan processor. The harness and children are pinned to CPU 0. Builds and agent work were idle during timing, and execution was outside ptrace. Each round runs all four engines; four Williams orders repeat eight times, balancing each execution position and every directed adjacent pair within a round. Cross-round predecessors are not balanced. Processes use their implementations' default randomized hash seeds; output does not depend on hash iteration order. Shared-host scheduling, frequency, allocator, and hash-seed variation remain possible. The ordinary wide/full run contains large retained outliers: maxima are 125.842 ms for Imre, 192.417 ms for Rust BigInt, and 127.375 ms for Rust i64. All 32 samples are retained when computing each median and quartile.

The timer surrounds process creation through exit and includes pipes and full I/O. Compilation, oracle evaluation, input generation, warmups, output comparison, and hashing are outside measured intervals. Every measured process must exit successfully, emit no stderr, and match the complete expected stdout. Each record retains the full input and expected output once per workload, plus every run's duration, order, exit status, stderr, stdout length, and SHA-256. Normal and ThinLTO runs were performed sequentially; their build modes are kept separate in the results.

CPython 3.14.5 is a release GIL build with PGO and LTO, with its optional JIT disabled. Rust is Ohm 1.98.1-dev (`f62703110`, LLVM 22.1.8), with Ohm's experimental defaults disabled. Imre emits LLVM IR through clang 18.1.3; ThinLTO uses LLD 22.1.8 and a runtime built with `-Clinker-plugin-lto`. Both ThinLTO implementations use LLVM import and inlining budgets of 1,000 and generic CPU targets. The build records preserve exact commands, environment overrides, tool versions, source text and hashes, and artifact hashes.

Rust BigInt uses the same pinned `num-bigint` 0.4.8 for durations and schedule arithmetic. Rust uses native `usize` for IDs and container positions, borrows stored task names for map keys, and uses its own native strings and containers. Checked i64 is a narrower numerical contract and rejects overflow explicitly. The comparison establishes performance for this program, algorithm, toolchain, and inputs; it does not isolate a language feature's cost or establish general language speed.

**Validation.** Before each timed comparison, all engines run against an independent oracle that never imports the compiled package. It uses `graphlib`, fixed-point affected-set discovery, `heapq`, and independent predecessor maxima. CPython, Imre, and Rust BigInt pass 155 graph cases totaling 1,712 input targets. Checked i64 matches 138 in-range graph cases and explicitly rejects all 17 overflow exclusions. Another 17 parser/framing cases cover 14 malformed inputs and three valid framing variants; all four engines pass them. Every large timed workload is also checked in full before and throughout measurement.

The final planner additionally passes the same 155 graph and 17 parser cases with both the Rust runtime and generated native code instrumented by AddressSanitizer, with LeakSanitizer enabled. This local check executes outside ptrace. CI's existing sanitizer suites instrument generated code; the linked local evidence also instruments the runtime. Local compiler/runtime validation passes 33 Rust unit tests and 517 frontend/native checks across the ordinary O0/O3 suites. The container suite passes 276 ordinary checks, 209 ThinLTO O3 checks, and 209 fully instrumented ASan O1 checks. Formatting and Clippy pass for the workspace and both Rust reference modes.

**Evidence and reproduction.** The measured implementation is clean commit [`347b8d0`](https://github.com/viarius-experiments/imre/commit/347b8d00ac0ce64fc4b4cbbf09e70f2f5f838d2f). First-party sources were explicitly refreshed and rebuilt before freezing compiler/runtime artifacts, avoiding stale shared-worktree build-cache results. The following files are exact copies of the frozen records; subsequent documentation/evidence commits do not change the measured implementation.

- [Ordinary comparison and raw samples](2026-10-17-incremental-planner-normal.json)
- [Matched ThinLTO comparison and raw samples](2026-10-17-incremental-planner-thinlto.json)
- [Frozen Imre source/build record](2026-10-17-incremental-planner-build.json)
- [Ordinary Rust source/build record](2026-10-17-incremental-planner-rust-normal-build.json)
- [ThinLTO Rust source/build record](2026-10-17-incremental-planner-rust-thinlto-build.json)
- [Final fully instrumented sanitizer build](2026-10-17-incremental-planner-sanitizer-build.json) and [complete sanitizer validation](2026-10-17-incremental-planner-sanitizer-validation.json)

The [reference guide](../reference/incremental_planner/README.md) gives ordinary and ThinLTO build commands and benchmark invocations, including local Ohm options. Supply the frozen Imre build record with `--imre-build-record`, use separate artifact directories for both modes, and retain `--samples 32 --warmups 2 --cpu 0` to reproduce this sampling configuration. The measured compiler, runtime, LLVM IR, native executables, and original records remain under the ignored `benchmarks/local/` paths alongside the frozen `incremental-*-final` build directories.
