> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Dependency planner: range proofs and specialized container access

The unchanged Python dependency planner now takes 20.1% less time on the large wide graph than the previous optimized Imre build. With matched ThinLTO settings, the gap to the arbitrary-precision Rust reference narrows from 45.6% to 16.3%. Imre is 6.67× faster than CPython on that workload. On the large layered graph, Imre takes 9.6% less time than the previous build and 11.4% less than the Rust BigInt reference.

These measurements use implementation revision `c0cb211cabbafb9d666bea4417a9a296a9856821`. All six Python package files and the Rust reference algorithm are unchanged. Integer precision, alias-visible mutation, error ordering, heap tie-breaking, scheduling, and complete report output retain their existing semantics.

## End-to-end results

Each cell is the median of 30 fresh-process runs in milliseconds. This main comparison enables ThinLTO for both Imre versions and both Rust references, with LLVM import and inline budgets of 1,000. Previous Imre is the preserved executable from [PR #12](https://github.com/viarius-experiments/imre/pull/12), with its matching build mode.

| Graph | Tasks | CPython 3.14.5 | Previous Imre | Imre | Rust BigInt | Rust checked-i64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Layered | 128 | 21.740 | 3.360 | 3.318 | 3.432 | 3.180 |
| Layered | 2,048 | 32.081 | 6.658 | 6.182 | 6.576 | 5.203 |
| Layered | 16,384 | 120.070 | 32.875 | 29.730 | 33.540 | 21.110 |
| Wide | 128 | 20.447 | 3.198 | 3.136 | 3.216 | 3.024 |
| Wide | 2,048 | 30.553 | 5.278 | 4.859 | 4.732 | 4.170 |
| Wide | 16,384 | 116.388 | 21.850 | 17.454 | 15.011 | 12.065 |

At 16,384 tasks, the layered/wide speedups over CPython are 4.04×/6.67×. Imre takes 0.886×/1.163× the Rust BigInt time, and 1.408×/1.447× the checked-i64 Rust time. The checked-i64 reference has a narrower numerical contract.

The wide Imre interquartile range is 16.764–17.875 ms, compared with 14.912–15.757 ms for Rust BigInt. The layered ranges are 29.491–30.074 ms and 32.722–34.062 ms. These describe observed spread on a shared host; they are not confidence intervals or a general language-performance claim. Small workloads remain dominated by process startup.

Ordinary linking also improves performance, using the original ordinary Rust release references:

| Graph | Tasks | CPython 3.14.5 | Previous Imre | Imre | Rust BigInt | Rust checked-i64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Layered | 128 | 20.495 | 3.298 | 3.279 | 3.362 | 3.097 |
| Layered | 2,048 | 31.840 | 6.909 | 6.541 | 6.935 | 5.399 |
| Layered | 16,384 | 122.486 | 35.837 | 32.755 | 36.564 | 23.407 |
| Wide | 128 | 20.412 | 3.189 | 3.165 | 3.247 | 3.012 |
| Wide | 2,048 | 30.478 | 5.550 | 5.184 | 4.948 | 4.235 |
| Wide | 16,384 | 117.639 | 24.870 | 20.891 | 16.638 | 12.954 |

The ordinary-link large wide workload takes 16.0% less time than the previous Imre build, but remains 25.6% slower than Rust BigInt. The optional ThinLTO configuration remains material to the main result. AOT compilation was measured separately: approximately 0.44 seconds for ordinary linking and 3.95 seconds for ThinLTO in these runs; these are individual build observations, not timing distributions.

## What changed

A conservative interval analysis follows immutable integer values through assignments, guards, short-circuit expressions, loop backedges, breaks, and continues. It specializes arithmetic only when both operands and the result fit the canonical small-integer representation. A `range` cursor proof includes captured arguments, zero iterations, and the final increment. Whole-function ownership proofs identify nonparameter integer locals that can never hold a BigInt. Missing facts, analysis overflow, unknown replacements, and potentially zero divisors keep checked lowering.

Proved nonnegative small indices use an exact decoded native-index ABI. Every access still checks the container's current bounds; no cached length or unchecked access is introduced. Negative and arbitrary-precision indices keep their normalization and error paths.

Two additional access specializations remove ownership work in heap operations. An immediate two-operand integer comparison can borrow a direct local list element only after both complete operand trees are proved nonmutating. Calls, mutators, fields, and chained comparisons retain owned reads. A same-local list copy can fuse its get and set only when both indices are simple, proved literals or locals; source bounds and acquisition precede destination checking and replacement, including self-copy.

Runtime storage uses scoped `UnsafeCell` access under the existing single-threaded, nonreentrant ABI. Mutable access is exclusive; storage references and buffer addresses cannot escape; displaced owners are dropped after access ends. The narrowly borrowed integer handle has its own rooting contract. Removing dynamic borrow flags saves eight bytes per affected container allocation. This relies on the absence of callbacks and finalizers, and does not introduce support for reentrant extensions or concurrent access. The rare BigInt comparison fallback also stays outside hot heap functions.

## CPU and allocation evidence

Callgrind and DHAT profile the exact final ThinLTO executable used above, with complete output checked against the captured workload. The cumulative instruction counts at 16,384 tasks are:

| Cumulative stage | Layered instructions | Wide instructions |
| --- | ---: | ---: |
| Previous optimized Imre ThinLTO | 205,147,254 | 145,225,371 |
| Range proofs and decoded indices | 194,584,138 | 121,441,037 |
| Scoped container storage | 182,650,490 | 106,504,878 |
| Proven LLVM overflow flags | 182,650,504 | 106,504,892 |
| Always-small local ownership | 181,108,491 | 104,396,037 |
| Cold BigInt comparison fallback | 180,707,653 | 103,372,563 |
| Borrowed comparisons and fused list copies | 179,989,365 | 99,436,272 |

The final build removes 12.3% of layered instructions and 31.5% of wide instructions relative to PR #12. Range/index specialization and scoped storage account for most of that reduction. The last access changes remove another 3,936,291 wide instructions relative to the preceding stage.

The overflow flags are justified by the proofs, but produced byte-identical `.text` sections in this program. Their 14-instruction difference comes entirely from the dynamic loader; it is not a generated-code improvement. The early ten-sample timing probes varied enough that this report uses the final 30-sample runs for performance claims. The stages are cumulative, not independent percentage contributions to wall time.

Allocation counts do not change in this follow-up: 233,897 blocks on the layered graph and 147,584 on the wide graph. Container header savings reduce requested bytes from 51,543,811 to 51,019,379 and from 10,078,143 to 9,553,711, respectively. Peak live bytes fall from 6,055,808 to 5,662,496 and from 5,130,272 to 4,736,960. Later stages preserve those allocation totals.

The matched Rust BigInt reference uses 196,808,173 layered instructions and 81,616,504 wide instructions. Imre still executes 21.8% more wide instructions. Wide allocation counts are already nearly equal—147,584 for Imre and 147,562 for Rust—but Imre requests 9,553,711 bytes versus Rust's 7,219,551, and peaks at 4,736,960 versus 3,720,814. Tagged values, generic container representations, and remaining checked accesses are still visible in the generated program. Local interval proofs do not establish the ranges of values loaded from containers or returned from functions; extending those proofs and specializing layouts are plausible next targets.

The final wide profile attributes 29.75 million exclusive instructions to the entry function, including inlined planning work. Whitespace splitting accounts for 7.96 million, `malloc` for 6.77 million, buffered output for 5.16 million, `free` for 4.57 million, and integer printing for 3.32 million. Parsing, allocation, and report generation remain substantial outside the heap operations. The emitted planner contains seven borrowed-read callsites and two fused-copy callsites; these optimizations are exercised by the real program.

The layered program still constructs its long critical-path string through repeated fresh prefix copies, preserving the same algorithm. Hardware-counter profiling was unavailable on this host. Callgrind counts executed instructions, not cycles or elapsed-time shares; DHAT measures requested allocations, not RSS. Instrumented durations are excluded from timing results.

## Correctness and scope

Both final comparisons pass 135 independent planner-oracle cases for CPython, both Imre versions, and Rust BigInt. Checked-i64 Rust passes the 121 representable cases and explicitly excludes 14 others. Ten additional boundary cases preserve arbitrary-precision behavior and verify four checked-i64 overflow rejections. Every warmup and measured process must match full output, exit successfully, and emit no stderr.

Formatting, Clippy with warnings denied, ten compiler unit tests, and twelve runtime unit tests pass. The five native suites pass 358 checks across ordinary O0/O3 and 263 checks at ThinLTO O3, plus ten ThinLTO CLI checks. The same five suites pass with both the Rust runtime and generated code instrumented by AddressSanitizer, using ordinary O1 and ThinLTO O3; normal exits are also checked for leaks. All twelve runtime unit tests pass with AddressSanitizer. The Rust standard library was not rebuilt with instrumentation.

The added cases cover BigInt boundaries, loop widening and cursor exits, local rebinding, alias mutation and shrinking, negative and enormous indices, RHS-first and augmented-assignment order, comparison snapshots, chained comparisons, owned container results, fused copy, self-copy, and reclamation. These checks support the closed subset's contracts; they are not a proof of general memory safety or an extension ABI.

Measurements ran outside ptrace on CPU 0 of an AMD EPYC-Milan devbox, with builds, tests, profilers, and agent tool work paused. Each of six workloads has two warmup rounds and 30 measured rounds. Ten Williams execution orders repeat three times, balancing positions and directed adjacent pairs within a round; cross-round predecessors are not balanced. Whole-process times include startup, pipes, parsing, scheduling, reporting, and exit. CPython also imports the unchanged package. Builds, correctness gates, warmups, and output comparisons are excluded.

The compiler/runtime use Ohm Rust 1.98.1-dev with LLVM 22.1.8. Generated IR enters through Clang 18.1.3; ThinLTO uses LLD 22.1.8. Imre and matched Rust ThinLTO builds use import/inlining budgets of 1,000. Ohm's experimental defaults were disabled. CPython 3.14.5 has its JIT disabled and records PGO/LTO build configuration. The Rust reference implements the same algorithm in different source, with native indices and a different representation. There is no C measurement or claim of general Rust parity.

## Evidence and reproduction

- [ThinLTO comparison](2026-10-17-dependency-index-specialization-lto.json): all raw samples, full inputs and expected outputs, validation, execution order, and source/tool/artifact hashes.
- [Ordinary-link comparison](2026-10-17-dependency-index-specialization-normal.json): the same evidence against ordinary-link Imre and Rust controls.
- [Cumulative profiles](2026-10-17-dependency-index-specialization-profiles.json): accepted trace summaries, complete-output checks, binary/build/source identities, and trace/log hashes, including the prior baseline and matched Rust controls.
- [Build and validation record](2026-10-17-dependency-index-specialization-build.json): clean implementation revision, source hashes, commands, toolchain, frozen artifact identities, and sanitizer scope.
- [Previous performance report](2026-10-17-dependency-performance.md): the preserved PR #12 baseline and reference provenance.

Raw Callgrind/DHAT traces, intermediate source snapshots, frozen executables, and exploratory timing records remain in ignored local directories. All accepted binaries match their recorded SHA256 values; an invalid diagnostic run interrupted by binary inspection is excluded. Final timing records were captured from the clean implementation revision above.

The [benchmark guide](../README.md) documents `dependency_compare.py` and `dependency_profile.py`. The [subset build instructions](../../docs/subset.md) describe ordinary and ThinLTO runtime builds. The [Rust reference guide](../reference/dependency_planner/README.md) describes the matching controls; use `CARGO_PROFILE_RELEASE_LTO=thin` and `benchmarks/reference/dependency_planner/build.py --llvm-budget 1000`, selecting `--toolchain ohm` on an Ohm development machine. Preserve the previous executable and its corresponding record when supplying `--baseline` and `--baseline-record`.
