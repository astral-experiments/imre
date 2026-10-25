> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Route planner: profiling and native optimization

The optimized compiler runs three routes on 256 × 256 grids in **74.17 ms**, compared with **192.89 ms** for the preserved original native executable and **419.76 ms** for CPython 3.14.5. That is **2.60× faster than the original native build and 5.66× faster than CPython**. All three execute the unchanged [`examples/route_planner.py`](../../examples/route_planner.py), with the same inputs, algorithm, validation, and complete output.

## Final comparison

These are medians from twelve measured triplets after two warmup triplets. Every invocation starts a fresh process. The six possible execution orders each occur twice per workload, balancing execution position and immediate predecessor. Each process solves three routes, using seeds 42, 43, and 44.

| Grid | CPython, ms | Original Imre, ms | Optimized Imre, ms | Original / optimized | CPython / optimized |
| --- | ---: | ---: | ---: | ---: | ---: |
| 32 × 32 | 22.87 | 5.58 | 4.00 | 1.40× | 5.72× |
| 128 × 128 | 109.60 | 46.93 | 19.42 | 2.42× | 5.64× |
| 256 × 256 | 419.76 | 192.89 | 74.17 | 2.60× | 5.66× |

For the largest workload, observed minimum–maximum times were 413.52–446.38 ms for CPython, 189.44–197.00 ms for original Imre, and 73.11–77.39 ms for optimized Imre. These ranges describe observed samples, not confidence intervals. Every optimized sample was faster than every original-native sample for its workload.

The [comparison JSON](2026-10-17-route-planner-comparison.json) retains all 108 measured executions and 18 warmups, their order, outputs, durations, hashes, host information, and build provenance. Both native executables also passed 22 validation inputs covering 64 routes against CPython and an independent Bellman-Ford oracle before timing. The [separate optimized-build record](2026-10-17-route-planner-optimized.json) records AOT compilation and an additional eleven-pair CPython comparison. Its large-workload result was 5.67×; the table above uses the direct three-way comparison throughout.

## What the profiles showed

Hardware performance counters are disabled on this devbox, so we used [Callgrind 3.22.0](https://valgrind.org/docs/manual/cl-manual.html) on the preserved and optimized executables. Both received `256`, `3`, and `42` on stdin and produced identical complete output. These are executed-instruction and call counts, **not CPU-cycle counts or elapsed-time shares**. Instrumented runs were separate from timing runs.

| Profile quantity | Original | Optimized |
| --- | ---: | ---: |
| Executed instructions | 1,574,194,732 | 699,985,140 |
| List retain/release calls, including null cleanup | 52,991,602 | 60 |
| Out-of-line index-normalization calls | 13,163,733 | 0, inlined |
| Generic integer-operation calls | 1,019,612 | 0 on this input |
| List reads | 9,259,554 | 9,259,554 |
| List writes | 3,904,179 | 3,904,179 |

The original compiler acquired and released a list reference around almost every use, and acquired another reference for every function parameter. Retain/release function bodies alone accounted for **22.97% of executed instructions**, with additional call-site work in generated code. Of the original release calls, 4,703,079 only released null during slot initialization or cleanup. Index normalization accounted for another **14.22%** in its out-of-line helper, beyond the caller's list-access work.

The final executable executes **55.53% fewer instructions**. Checked list reads and writes now account for **48.13%** of its instructions; generated heap/Dijkstra control flow and arithmetic account for most of the rest. List-access counts are unchanged. The generic integer fallback remains implemented and tested; this particular workload's integer values fit the tagged representation, so its calls disappear after inlining the small-integer paths.

The [profile JSON](2026-10-17-route-planner-profile.json) includes complete function aggregates, input/output and binary hashes, and links to compressed traces for the original, intermediate, and final executables. Environment-specific source/object path prefixes are normalized to `/imre/`, `/cargo-registry/`, and `/valgrind/`; instruction counts, call records, and symbols are unchanged. Hashes identify both the published traces and the original local captures. Source-location rows are summed by function, excluding inclusive call-cost records, and the resulting function totals equal each trace's instruction total. `imre_list_retain` and `imre_str_retain` share a machine-code address; the profiler labels these list-retain calls with the string-retain symbol. Inlining also changes which generated function contains the route-planner work.

## Changes and their evidence

The largest change is explicit borrowing during ownership lowering. Operators and calls borrow direct locals whose existing roots remain alive throughout expression evaluation. Parameters whose bindings are never reassigned share the caller's root. Assignments and returns still acquire or transfer ownership, and list-element reads remain owned across later mutation. This follows from the enforced subset: expressions cannot rebind a caller's local slots. It does not imply that list objects are immutable or unaliased. Removing this bookkeeping also makes more first-party helper functions small enough for LLVM to inline.

The runtime now inlines index normalization into checked get/set operations, allowing LLVM to remove a redundant bounds check. Negative indices use wrapping addition followed by the existing bounds check: an underflow wraps above the list length and is rejected. The runtime continues to own the resizable buffer, with no layout offsets or buffer pointers exposed in generated LLVM.

Small-integer floor division and modulo now have inline LLVM paths. They guard zero before division, apply Python's floor/remainder sign rules, and promote `-2**62 // -1` to arbitrary precision. BigInt inputs and errors use the existing runtime fallback. This also lets LLVM optimize constant divisors in the heap and terrain generator.

Exploratory six-triplet comparisons guided these changes:

| Change | Compared with | Median before → after, ms | Improvement |
| --- | --- | ---: | ---: |
| Index inlining alone | Original native | 193.18 → 176.26 | 1.10× |
| Borrowed locals and parameters alone | Original native | 191.22 → 97.92 | 1.95× |
| Borrowing plus index inlining | Original native | 191.13 → 79.85 | 2.39× |
| Simpler negative-index normalization | Previous combined build | 79.41 → 77.17 | 1.03× |
| Inline small-integer floor/modulo | Previous normalized-index build | 77.29 → 73.56 | 1.05× |

These [exploratory records](2026-10-17-route-planner-ablations.json) identify intermediate binaries by hash and record dirty worktrees. The last exploratory binary differs from the subsequent clean rebuild; its result is supplementary rather than the final performance claim. The final comparison uses verified build records and the exact executable profiled above. The ratios from different exploratory runs should not be multiplied as an exact decomposition.

## Semantics, memory reclamation, and environment

The differential suite passed **92 checks at O0/O3** and **62 checks at O1 with generated code and the Rust runtime crate instrumented by AddressSanitizer**, with LeakSanitizer enabled. Coverage includes 260 integer pairs, constant divisors at representation boundaries, runtime-produced zero divisors, negative and oversized indices, and BigInt values removed or replaced by later argument evaluation. Alias assignment, returns, comparison chains, conditional parameter rebinding, and loop-target rebinding are covered. The instrumented route planner also passed all 64 oracle-checked routes. Formatting, Clippy, and the runtime unit tests passed. The sanitizer build instruments the runtime crate and its dependencies; the prebuilt Rust standard library is not instrumented.

Memory reclamation remains reference counting. The supported heap graph is acyclic because lists contain integers; a tracing or cycle collector is unnecessary for these types. General recursive containers and user-defined object graphs will require a collection design. This optimization removes redundant ownership operations while retaining the roots needed for safe reclamation.

The measured optimized revision is [`2f8687ab3488445ded4554f44c8a9b1267963b0e`](https://github.com/viarius-experiments/imre/commit/2f8687ab3488445ded4554f44c8a9b1267963b0e), with a clean worktree. The original executable is byte-identical to the one recorded at [`312557ad1cfc94fcff05741f328c2596273a3f2c`](https://github.com/viarius-experiments/imre/commit/312557ad1cfc94fcff05741f328c2596273a3f2c). The first report and its raw data are preserved.

Both implementations ran pinned to CPU 0 on the same AMD EPYC-Milan devbox, x86-64 Linux `6.8.0-146-generic`, outside the tracing sandbox and after local builds finished. CPython 3.14.5 was non-debug, GIL-enabled, with JIT disabled and PGO/LTO configuration reported. Imre used Ruff/ty 0.0.14 and Clang/LLVM 18.1.3 at O3. The Rust compiler/runtime used Ohm `1.98.1-1`, Rust `1.98.1-dev` (`f6270311094cd4b48fefce03debdffcf8396c64c`), and Rust's LLVM 22.1.8 backend, with Ohm's experimental Cargo defaults disabled.

AOT compilation took **0.464 seconds**, excluding the Rust workspace build. The final executable is **2,381,192 bytes**, and emitted LLVM IR is **45,442 bytes**. The [reproduction instructions](../README.md) describe both comparison and profiling commands. Timings include startup, stdin/stdout pipes, route generation, solving, validation, and process exit. This is a shared host without exclusive CPU ownership or fixed frequencies, and these results apply to this workload and toolchain.
