> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Removing repeated string copies from unchanged Base45

With the upstream Python codec unchanged, sustained ThinLTO encoding now takes **1.535× Rust's time for 16 KiB inputs**, **1.676× for 1 KiB**, and **2.406× for 64 bytes**. The large-input encoder is **43.96× faster than the previous Imre executable** in a separate same-session comparison. This substantially narrows the native gap; it does not establish Rust parity. ThinLTO decoding still takes **26.95–31.51× Rust's time**.

The [compiler/runtime change](https://github.com/viarius-experiments/imre/pull/49) continues GitHub Stack 4 after the unchanged-library milestone. Measured application, Rust reference, and harness sources are clean commit `3349b85efc78e5ac09a92d8d367d914264679067`; compiler/runtime sources are `31bffb7e1aa929497c89b6de38f8e9efa83302e8`. This report and its evidence are committed afterward. The [initial report](2026-10-19-base45.md) remains intact.

## Sustained comparison with Rust

Each process encodes **64 MiB of raw data** through repeated calls on the indicated payload. Values are median milliseconds with inclusive quartiles in brackets. **Imre / Rust is a time ratio: lower is better.** Both native executables use ThinLTO, including matched LLVM import and inline budgets of 1,000. Twenty balanced AB/BA measured rounds follow two balanced warmup rounds.

| Raw bytes per call | Imre ms [Q1–Q3] | Rust ms [Q1–Q3] | Imre / Rust |
|---|---:|---:|---:|
| 64 | 462.438 [460.650–464.695] | 192.214 [191.288–193.173] | 2.406× |
| 1,024 | 294.293 [293.460–296.674] | 175.558 [174.758–176.571] | 1.676× |
| 16,384 | 268.504 [267.842–269.280] | 174.893 [173.903–175.983] | 1.535× |

These larger batches reduce the proportion of process startup in the samples. The shorter 2 MiB comparisons below put the 16 KiB case at 1.365× Rust's time; the sustained **1.535×** result is the more useful assessment of the remaining encoder gap.

The [independent Rust reference](../reference/base45_codec/README.md) uses a fixed alphabet, preallocated byte buffers, direct arithmetic, and the same file protocol, repetition count, checksum, and complete final output. `black_box` guards each input and result against elimination of repeated work. The reference's exact source, compiler, flags, and executable hashes are retained in every benchmark record. This compares the unchanged Python implementation with an idiomatic native implementation, including their different storage strategies.

## Same-session comparison with previous Imre

The previous and current retained ThinLTO executables were interleaved on CPU 0 using identical inputs and the same 2 MiB batch protocol. Twenty balanced AB/BA rounds follow two warmup rounds. The original codec and driver hashes match across both executables; every output and checksum is checked.

| Raw bytes per call | Previous Imre ms [Q1–Q3] | Current Imre ms [Q1–Q3] | Previous / current |
|---|---:|---:|---:|
| 64 | 221.874 [220.441–223.305] | 18.229 [17.173–18.699] | 12.171× |
| 1,024 | 262.729 [261.842–263.999] | 13.058 [11.850–13.334] | 20.120× |
| 16,384 | 515.383 [513.044–519.710] | 11.723 [11.293–12.350] | 43.963× |

Current samples in this control are short and noisier: the largest IQR/median is 11.36%. Their spread is shown above. The control directly establishes the before/after encoder improvement; the separately measured Rust comparisons establish the remaining native gap.

## Ordinary linking: Imre, Rust, and CPython

Each row represents **2 MiB of raw data** per process, with six balanced warmup rounds and 24 measured rounds covering all six engine orders four times. **CPython / Imre is a speedup ratio: higher is better.**

| Operation / raw bytes per call | Imre ms [Q1–Q3] | Rust ms [Q1–Q3] | CPython ms [Q1–Q3] | Imre / Rust | CPython / Imre |
|---|---:|---:|---:|---:|---:|
| encode-64 | 23.274 [23.116–23.783] | 9.444 [9.315–9.550] | 328.516 [326.976–334.146] | 2.465× | 14.115× |
| decode-bytes-64 | 279.419 [278.156–283.967] | 6.987 [6.858–7.049] | 327.920 [326.188–329.812] | 39.989× | 1.174× |
| decode-text-64 | 278.632 [278.099–281.066] | 6.508 [6.415–6.634] | 324.203 [322.409–326.734] | 42.814× | 1.164× |
| encode-1024 | 16.793 [16.620–17.069] | 8.763 [8.636–8.879] | 338.788 [337.995–340.897] | 1.916× | 20.174× |
| decode-bytes-1024 | 264.623 [263.520–265.570] | 6.170 [6.012–6.344] | 316.879 [315.801–319.020] | 42.889× | 1.197× |
| decode-text-1024 | 264.960 [264.320–268.821] | 6.137 [5.965–6.213] | 315.811 [313.659–317.572] | 43.177× | 1.192× |
| encode-16384 | 15.769 [15.612–15.993] | 8.782 [8.696–8.906] | 341.659 [340.221–343.160] | 1.796× | 21.667× |
| decode-bytes-16384 | 289.578 [288.742–292.338] | 6.299 [6.158–6.480] | 341.114 [339.720–346.188] | 45.973× | 1.178× |
| decode-text-16384 | 266.315 [264.943–267.576] | 5.948 [5.843–6.097] | 338.421 [335.883–341.188] | 44.777× | 1.271× |

## ThinLTO: Imre, Rust, and CPython

| Operation / raw bytes per call | Imre ms [Q1–Q3] | Rust ms [Q1–Q3] | CPython ms [Q1–Q3] | Imre / Rust | CPython / Imre |
|---|---:|---:|---:|---:|---:|
| encode-64 | 18.187 [17.861–18.246] | 9.447 [9.309–9.649] | 329.143 [326.614–330.514] | 1.925× | 18.097× |
| decode-bytes-64 | 179.764 [179.148–181.140] | 6.671 [6.496–6.743] | 328.746 [327.184–332.038] | 26.948× | 1.829× |
| decode-text-64 | 177.509 [176.730–179.217] | 6.356 [6.190–6.472] | 326.561 [324.374–328.398] | 27.927× | 1.840× |
| encode-1024 | 12.729 [12.611–12.878] | 8.813 [8.696–8.920] | 341.085 [338.770–342.585] | 1.444× | 26.796× |
| decode-bytes-1024 | 167.141 [165.660–168.302] | 5.954 [5.837–6.200] | 320.561 [317.089–323.748] | 28.073× | 1.918× |
| decode-text-1024 | 164.735 [163.694–165.770] | 5.860 [5.704–5.956] | 319.390 [315.876–322.438] | 28.113× | 1.939× |
| encode-16384 | 12.137 [11.948–12.260] | 8.891 [8.736–9.054] | 343.601 [340.295–346.503] | 1.365× | 28.310× |
| decode-bytes-16384 | 190.964 [190.625–192.313] | 6.060 [5.905–6.198] | 343.737 [339.081–346.345] | 31.514× | 1.800× |
| decode-text-16384 | 165.315 [164.330–166.232] | 5.998 [5.807–6.083] | 343.405 [339.165–348.503] | 27.560× | 2.077× |

ThinLTO encoding is **18.10–28.31× faster than CPython** in these batches. Decoding remains far from the Rust reference even though it is faster than CPython. Ordinary and ThinLTO families ran separately, each with interleaved baselines; these are comparisons within each family, not a randomized measurement of ThinLTO's isolated effect. Quartiles describe observed spread, not confidence intervals.

## What changed

The compiler recognizes eligible self-appends such as `result = result + first + second + third`. It evaluates suffixes while the old value remains rooted, then transfers the local's ownership to a consuming append helper. Uniquely owned, disjoint storage can grow geometrically and be reused. Shared ownership, weak references, and overlapping fragments take a copying fallback, preserving aliases, evaluation order, Unicode behavior, and exceptions. The original whole-prefix copy on every iteration is removed.

String-index fragments can be written into small UTF-8 scratch buffers without allocating a temporary string object. Proven in-bounds accesses to immutable ASCII literals or module values use direct byte loads; adjacent fragments can share one append. Other strings and indices retain the general Unicode and error paths.

A guarded range pass specializes eligible zero-start, positive-constant-step leaf loops. It evaluates the stop once, proves the small-integer limit before entering the fast loop, and uses a private cursor while retaining the visible Python loop target. The original arbitrary-precision loop remains the fallback. Proven byte indices and canonical small-integer facts let LLVM remove generic integer work and unnecessary checks. Discarded scalar literals, including docstrings, no longer allocate at runtime. These changes are generic compiler/runtime optimizations; the codec source is unchanged.

## Allocation and instruction profiles

Separate DHAT and Callgrind runs use the exact timed ThinLTO Imre and Rust binaries. Each profiled process handles **64 KiB of raw data**: 1,024 calls for encode-64 or four calls for the large cases. Inputs, repetitions, checksums, and full outputs match the historical profiles.

| Case / engine | Cumulative allocated bytes | Allocation blocks | Peak live bytes | Callgrind instructions |
|---|---:|---:|---:|---:|
| encode-64 / previous Imre | 7,046,310 | 160,854 | 14,379 | 66,583,941 |
| encode-64 / current Imre | 503,922 | 7,252 | 14,414 | 6,136,042 |
| encode-64 / Rust | 102,474 | 1,042 | 2,170 | 2,280,287 |
| encode-16384 / previous Imre | 408,084,816 | 163,914 | 95,174 | 439,795,671 |
| encode-16384 / current Imre | 557,892 | 168 | 105,386 | 3,337,720 |
| encode-16384 / Rust | 118,797 | 22 | 67,453 | 2,060,679 |
| decode-bytes-16384 / previous Imre | 7,291,988 | 98,535 | 455,557 | 70,934,097 |
| decode-bytes-16384 / current Imre | 7,291,628 | 98,525 | 455,592 | 70,473,196 |
| decode-bytes-16384 / Rust | 94,239 | 22 | 59,279 | 1,618,712 |

For large encoding, cumulative allocation falls from **408,084,816 to 557,892 bytes**, a **731× reduction**; allocation blocks fall **976×**, and instrumented instructions fall **132×**. Peak live allocation rises from **95,174 to 105,386 bytes**, or **10.7%**, because geometric capacity growth retains some spare space. Cumulative allocation is not peak memory or RSS.

The remaining large-encode allocation includes **418,252 bytes in 72 growth allocations** under the append slow path and **98,304 bytes in four final string-to-bytes conversions**. Growth is linear overall, but Rust reserves the final output capacity directly. Current Imre still requests **4.70×** Rust's cumulative allocation and executes **1.62×** its instrumented instructions in this profile. These counts include imports, startup, file I/O, and checksum work. Callgrind counts are instrumented instructions, not hardware CPU-time percentages. Profiling ran separately from timing.

The decoder's allocation and instruction counts remain close to the historical profile. Its **98,525 allocation blocks** and **70.47 million instructions**, versus Rust's **22 blocks** and **1.62 million instructions**, make it a distinct remaining optimization target. This encoder change does not resolve that gap.

## Semantics and validation

The codec, `py.typed`, upstream tests, and BSD-2-Clause license match all four hashes in the [upstream manifest](../../examples/base45_codec/UPSTREAM.json), pinned to [python-base45 commit 5f49a519](https://github.com/kirei/python-base45/tree/5f49a519c3884a4fbc4c58c5c8f1fa25d7046020). There is no source rewrite or CPython fallback. The separate upstream `argparse` application is outside this example.

At each of five optimization configurations, all five unchanged upstream test methods pass through the native adapter, covering **323 validly typed codec calls**, **289 additional CPython differential cases**, and **two compile-time rejections** for upstream calls passing integer `42` to typed APIs. Those last two checks are not native runtime `TypeError` tests. Validation checks complete values, exact error messages, causes, suppression, unchanged output on failure, alias mutations, and recovery, including odd, empty, malformed, Unicode, and all-byte inputs.

The Rust reference passes **295 checks** against both CPython 3.14.5 and the CI-compatible Python 3.12 interpreter. Of these, 291 require exact messages; four explicitly recorded combining-mark `repr` boundaries allow different character quoting while checking error category, cause, suppression, and unchanged output. Imre matches CPython's exact messages in all cases. Timed decoder workloads use valid ASCII and do not exercise those four Rust diagnostic boundaries.

The integrated changes pass **116 Rust unit tests**, strict workspace Clippy, formatting, focused Ruff checks, and **23 language suites** across ordinary O0/O3, ThinLTO O3, and ASan/LeakSanitizer O0/O1. Excluding separately reported Base45 checks, this is **3,892 language checks**: 1,414 ordinary, 1,064 ThinLTO, and 1,414 combined sanitizer checks. Counts include compile-time rejections. One ThinLTO float compilation timed out during a 24-worker validation run; its isolated retry passed. Both attempts are retained. Sanitizers cover generated code and the custom runtime with leak detection enabled; the prebuilt Rust standard library is uninstrumented.

All six existing first-party application oracle suites pass **935 cases**, plus **135 additional dependency-graph replays against Rust**. The [validation index](2026-10-19-base45-string-reuse-validation.json) retains the outcomes, commands where recorded, counts, and provenance.

## Measurement and retained evidence

All timing ran on October 19, 2026, on an AMD EPYC-Milan host pinned to CPU 0, outside tracing. CPython 3.14.5 has PGO and LTO enabled, the GIL enabled, and JIT disabled. Imre uses Clang 18 at O3; Rust uses Ohm 1.98.1 with LLVM 22.1.8. ThinLTO uses LLVM 22 LLD and linker-plugin runtime bitcode. The [build manifest](2026-10-19-base45-string-reuse-build.json) records exact tool versions, commands, source hashes, and frozen artifact identities. Compiler/runtime artifacts were built before their commit from the same verified source bytes; measurement sources were committed and clean.

Timing includes process startup, imports where applicable, one input read, all repeated codec calls, checksum work, one complete final output write, and exit. Preparation, compilation, hashing, and verification are outside the clock. OS caches are warm and output is not fsynced. Task-owned builds, tests, and profiles were stopped during timing; host frequency and unrelated shared-host activity remain uncontrolled. Timed inputs have valid even lengths; separate correctness checks cover the other cases.

Across all four timing records, **1,536 measured executions, 348 warmups, and 66 gates** retain exit status, stdout/checksum, stderr, and complete output hashes. Input, source, upstream, tool, and executable identities are guarded before and after timing. The [independent benchmark audit](2026-10-19-base45-string-reuse-audit.json) recomputed all statistics, verified balanced ordering and correctness counts, and checked all **1,950 process results** and **82 retained files**. The [independent profile audit](2026-10-19-base45-string-reuse-profile-audit.json) verified all **12 profiles**, **142 retained/guarded files**, raw totals, timed-binary identities, and regenerated annotations. Neither audit found discrepancies.

Raw records are available for [ordinary linking](2026-10-19-base45-string-reuse-normal.json.gz), [ThinLTO](2026-10-19-base45-string-reuse-thinlto.json.gz), [sustained native encoding](2026-10-19-base45-string-reuse-sustained.json.gz), and the [previous/current control](2026-10-19-base45-string-reuse-before-after.json.gz). The [profile summary](2026-10-19-base45-string-reuse-profile.json) and [historical comparison](2026-10-19-base45-string-reuse-profile-comparison.json) retain detailed allocation and instruction results.

The [evidence archive](2026-10-19-base45-string-reuse-evidence.tar.gz) and [manifest](2026-10-19-base45-string-reuse-evidence-manifest.json) retain measured source and fixtures, generated LLVM, raw records, validation logs, profiles, build records, and reproduction/audit scripts. Native executables, runtime archives, interpreters, and toolchains remain external and hash-identified. The [example guide](../../examples/base45_codec/README.md) and [Rust reference guide](../reference/base45_codec/README.md) provide reproduction commands; recorded original commands retain their host-specific paths.
