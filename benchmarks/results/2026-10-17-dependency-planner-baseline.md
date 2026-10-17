> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Dependency planner: package baseline

On 2026-10-17, Relar ran the unchanged [dependency-planner package](../../examples/dependency_planner/README.md) **3.44–3.51× faster than CPython at 128 tasks**, but took **4.43–4.48× as long at 16,384 tasks**. This baseline establishes working compilation of static modules, typed classes, general lists, tuples, and optional values; the larger workloads expose a performance problem for subsequent profiling.

Each process parses a graph, computes a deterministic topological schedule and earliest completion times, reconstructs a critical path, performs a named-task lookup, and prints the complete report. Layered graphs have up to four dependencies per task; wide graphs have no dependencies. Both implementations execute the same source and input, with seed 42.

The table uses twelve measured pairs after two warmup pairs per workload. Parentheses show the observed minimum–maximum. The final column is CPython's median divided by Relar's median; values below one mean Relar is slower.

| Shape | Tasks | Dependencies | CPython median (range), ms | Relar median (range), ms | CPython / Relar |
| --- | ---: | ---: | ---: | ---: | ---: |
| Layered | 128 | 295 | 20.36 (20.25–22.96) | 5.91 (5.69–6.91) | 3.443× |
| Layered | 2,048 | 4,821 | 32.48 (31.57–35.68) | 72.10 (69.65–73.27) | 0.451× |
| Layered | 16,384 | 38,622 | 124.62 (121.12–138.90) | 558.83 (550.87–585.67) | 0.223× |
| Wide | 128 | 0 | 20.17 (19.89–22.65) | 5.75 (5.43–8.56) | 3.506× |
| Wide | 2,048 | 0 | 32.75 (30.21–33.49) | 67.99 (66.89–71.21) | 0.482× |
| Wide | 16,384 | 0 | 118.10 (116.68–124.24) | 523.30 (520.33–529.56) | 0.226× |

The native advantage reverses between the measured 128- and 2,048-task sizes. Startup and Python imports contribute to these whole-process timings, so the small-input ratio does not establish scheduling throughput. These measurements do not isolate the source of the larger-input slowdown.

The timer surrounds `subprocess.run` with `time.perf_counter_ns`: each sample includes fresh process startup, stdin/stdout pipes, parsing, scheduling, complete report output, and process exit. CPython runs with `-I` and an explicit package-root bootstrap using `runpy.run_module`, equivalent to executing the unchanged package with `-m`; normal imported-module bytecode caching is allowed. Relar runs a previously compiled native executable. Execution order alternates, giving each implementation the first position six times per workload. The harness and children were pinned to CPU 0, outside ptrace tracing.

AOT compilation at LLVM O3 took **0.502 seconds**, separately measured with the Rust compiler and runtime already built. The executable is **2,469,880 bytes** and emitted IR is **94,247 bytes**. Compilation, correctness checks, warmups, output comparison, and output hashing are excluded from measured execution samples.

Before timing, **135 oracle cases covering 1,318 input tasks** passed: 95 valid plans, 22 cycles, 16 missing-dependency errors, and two negative-duration errors. The independent validator uses `graphlib`, exhaustive ready-set ordering, and dependency maximums; it checks complete output, critical-path ties, error precedence, huge integers, duplicate edges, Unicode labels, and whitespace handling. The same 135 cases separately passed AddressSanitizer and LeakSanitizer checks with generated code and the Rust runtime instrumented. The prebuilt Rust standard library was not rebuilt with instrumentation.

Every one of the **144 measured executions and 24 warmups** exited successfully, emitted no stderr, and matched the complete expected output. The [unmodified raw JSON](2026-10-17-dependency-planner-baseline.json) stores all raw durations and orders, full inputs and expected outputs, per-run output hashes and byte lengths, tool/artifact/source hashes, compilation metadata, and quartiles. An independent record audit verified those hashes against the preserved artifacts and revision, checked every output report's schedule and critical path, and recomputed every summary and ratio from raw samples.

The measured revision is [`81cba9801bf79bd9ff6c2008c96f453c1bd98fbf`](https://github.com/viarius-experiments/relar/commit/81cba9801bf79bd9ff6c2008c96f453c1bd98fbf), with a clean worktree. The host reports AMD EPYC-Milan, x86-64 Linux `6.8.0-146-generic`. CPython 3.14.5 was non-debug and GIL-enabled, with JIT available but disabled and PGO/LTO configuration reported. Relar used Ruff/ty 0.0.14 and Clang/LLVM 18.1.3. The compiler and runtime were built with Ohm `1.98.1-1`, Rust `1.98.1-dev` (`f6270311094cd4b48fefce03debdffcf8396c64c`), with Ohm’s experimental Cargo defaults disabled.

To reproduce this baseline, check out the measured revision above before building the workspace. Follow the [package instructions](../../examples/dependency_planner/README.md) and run:

```sh
python3.14 benchmarks/dependency_benchmark.py \
  --compiler target/release/relar \
  --runtime target/release/librelar_runtime.a \
  --python python3.14 --clang clang-18 \
  --sizes 128 2048 16384 --shapes layered wide \
  --seed 42 --samples 12 --warmups 2 --cpu 0 \
  --output benchmarks/local/dependency-planner-baseline.json
```

Observed ranges are not confidence intervals. This shared devbox had no exclusive CPU ownership or fixed frequency controls. Startup, imports, report size, scheduler interference, and memory/cache contention affect the result. The measurements apply to these inputs and toolchain, and do not establish a general Python speedup.
