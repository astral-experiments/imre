> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Dependency planner: complete package and buffered output

The compiled [dependency-planner package](../../examples/dependency_planner/README.md) schedules and reports 16,384 tasks in **51.36 ms for a layered graph** and **33.87 ms for independent tasks**. In the same run, those results were **2.39× and 3.50× faster than CPython 3.14.5**, and **10.97× and 15.39× faster than the preserved native baseline**. All three execute the unchanged Python package with identical inputs and complete output.

The program uses static imports, typed classes and methods, lists of objects, nested lists, fixed tuples, optional returns, and strings. It parses task descriptions, validates dependencies, computes a deterministic topological order and earliest completion times, reconstructs a critical path, performs a named lookup, and prints every task's result. Dependency IDs permit semantic graph cycles without cyclic heap ownership. The [first baseline report](2026-10-17-dependency-planner-baseline.md) preserves the original large-input slowdown.

## Direct comparison

These are medians from twelve measured triplets after two warmup triplets per workload. Each of the six execution orders occurs twice, balancing every implementation's position and predecessor within triplets. Every invocation starts a fresh process. Layered graphs contain 295, 4,821, and 38,622 dependencies at the respective sizes; wide graphs contain none. All inputs use seed 42.

| Shape | Tasks | CPython, ms | Baseline Imre, ms | Buffered Imre, ms | Baseline / buffered | CPython / buffered |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Layered | 128 | 20.35 | 5.80 | 3.42 | 1.70× | 5.96× |
| Layered | 2,048 | 31.50 | 70.15 | 8.33 | 8.42× | 3.78× |
| Layered | 16,384 | 122.81 | 563.70 | 51.36 | 10.97× | 2.39× |
| Wide | 128 | 20.17 | 5.60 | 3.25 | 1.73× | 6.22× |
| Wide | 2,048 | 30.62 | 66.74 | 6.55 | 10.19× | 4.68× |
| Wide | 16,384 | 118.52 | 521.21 | 33.87 | 15.39× | 3.50× |

For the largest layered graph, observed minimum–maximum times were 121.01–135.00 ms for CPython, 552.70–571.50 ms for baseline Imre, and 50.32–53.41 ms for buffered Imre. For the largest wide graph, they were 116.08–136.07, 517.04–533.15, and 32.80–35.07 ms. These ranges describe the observations, not confidence intervals.

The [comparison JSON](2026-10-17-dependency-planner-comparison.json) retains **216 measured executions and 36 warmups**, every order and raw duration, full inputs and expected outputs, and per-run output SHA256 and byte length. Every execution exited zero, emitted no stderr, and matched the entire expected report before its compact record was retained. An independent audit verified the source/tool/artifact hashes, baseline provenance, all six execution orders, output identities, and every median, quartile, range, and ratio.

## What changed

Profiling the baseline showed tens of thousands of native `write` system calls while printing the report. The runtime now uses a bounded 8 KiB stdout buffer for pipes and files. Terminals still flush each printed line. Explicit flushes preserve output before `input()`, fatal errors, and normal program exit; returned write or flush errors produce a runtime error, while a secondary flush failure preserves the original fatal error category. Native signal handling is unchanged: a closed output pipe can terminate either native build with `SIGPIPE` before an I/O error is returned.

Separate syscall profiles of the exact benchmark executables show the reduction:

| 16,384-task workload | Baseline Imre writes | Buffered Imre writes | CPython writes |
| --- | ---: | ---: | ---: |
| Layered | 83,574 | 76 | 5 |
| Wide | 81,934 | 61 | 4 |

The [profile JSON](2026-10-17-dependency-planner-profile.json) records binary/input/output identities, syscall summaries, and separate untraced CPU/context-switch observations. Profile outputs match the captured benchmark output byte for byte. Syscall tracing is diagnostic and changes execution cost; its timing is not used for the speedups above. The algorithm and all six Python source files are unchanged.

## Correctness and timing boundaries

Before measurement, both native executables independently passed **135 oracle cases covering 1,318 input tasks** against CPython: 95 valid plans, 22 cycles, 16 missing dependencies, and two negative durations. The independent oracle uses `graphlib`, ready-set ordering, and dependency maximums. It checks complete schedules, earliest times, critical-path ties, error precedence, huge integers, duplicate edges and labels, and Unicode/control whitespace.

The buffered implementation also passed all 135 planner cases with generated code and the Rust runtime instrumented by AddressSanitizer, with LeakSanitizer enabled. Instrumented class and container suites passed 56 and 53 checks. Output-specific tests passed 20 O0/O3 checks against each of CPython 3.12 and 3.14, plus ten O1 checks with the runtime instrumented. They cover pipe handshakes, terminal visibility, normal and fatal exit output, Unicode/NUL/large writes, and write/flush failures. The prebuilt Rust standard library was not rebuilt with instrumentation.

The timer surrounds `subprocess.run` with `time.perf_counter_ns`, including process startup, stdin/stdout pipes, parsing, scheduling, the complete report, and process exit. CPython uses `-I` and an explicit package-root `runpy.run_module` bootstrap, equivalent to running the unchanged package with `-m`; normal imported-module bytecode caching is allowed. Native executables are compiled before timing. Rust workspace builds, AOT compilation, oracle validation, warmups, and output comparison/hashing are excluded from measured executions.

The current AOT compilation took **0.511 seconds**, separately measured with the compiler and Rust runtime already built. The buffered executable is **2,496,472 bytes** and its LLVM IR is **94,314 bytes**. Baseline provenance is verified by its original record and executable hash, rather than inferred from the current compiler.

The measured revision is [`bee1ac80c7f3f196711ddb603c3170329557411d`](https://github.com/viarius-experiments/imre/commit/bee1ac80c7f3f196711ddb603c3170329557411d), with a clean worktree. The preserved baseline comes from clean revision [`81cba9801bf79bd9ff6c2008c96f453c1bd98fbf`](https://github.com/viarius-experiments/imre/commit/81cba9801bf79bd9ff6c2008c96f453c1bd98fbf). Imre used Ruff/ty crates 0.0.14 and Clang/LLVM 18.1.3 at O3. The Rust compiler/runtime build used Rust `1.98.1-dev` (`f6270311094cd4b48fefce03debdffcf8396c64c`) from Ohm `1.98.1-1`, whose Rust backend is LLVM 22.1.8; that is separate from LLVM 18 compiling the generated Python-program IR. CPython 3.14.5 was non-debug and GIL-enabled, with JIT available but disabled and PGO/LTO configuration reported.

The harness and all children ran pinned to CPU 0 on an AMD EPYC-Milan devbox, x86-64 Linux `6.8.0-146-generic`, outside ptrace tracing. This shared host has no exclusive CPU ownership or fixed frequency controls. Startup, imports, report size, scheduling interference, and cache/memory contention affect the measurements. These are whole-program results for these graphs and toolchain, not isolated scheduling throughput or a general Python speedup. Reproduce them with the [package's three-way comparison instructions](../../examples/dependency_planner/README.md), using sizes 128/2,048/16,384, layered and wide shapes, seed 42, twelve samples, two warmups, and CPU 0.
