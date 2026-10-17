# Route planner benchmark

The example computes shortest routes across weighted grids using Dijkstra's algorithm and its own binary heap. CPython and Relar execute the same `examples/route_planner.py` source, receive the same input, and must produce exactly the same output. The compiler generates LLVM IR and links a native executable against the independent Rust runtime. There is no rewritten native kernel or CPython interpreter inside that executable.

The [first recorded result](results/2026-10-17-route-planner.md) includes raw samples and environment details: 2.17× faster than CPython 3.14.5 for three routes on 256 × 256 grids, using end-to-end process timings.

Build the compiler and runtime once, then run the benchmark from the repository root:

```sh
cargo build --release --workspace --locked
python3.14 benchmarks/benchmark.py \
  --compiler target/release/relar \
  --runtime target/release/librelar_runtime.a \
  --python python3.14 \
  --clang clang \
  --output benchmarks/local/route-planner.json
```

Use the repository's pinned Rust toolchain and LLVM 18 or a compatible Clang installation. On a development machine configured to require Ohm, use `cargo +ohm build --release --workspace` instead and pass the compiler/runtime paths from that checkout's `CARGO_TARGET_DIR`. The harness itself requires Python 3.11 or newer; `--python` independently selects the CPython executable being measured. The current harness requires Linux for CPU affinity and its tracing check.

The default workloads use grid widths 32, 128, and 256, with three routes per process and seed 42. `--sizes`, `--routes`, and `--seed` select other workloads. Each workload has two warmup pairs followed by eleven measured pairs; `--warmups` and `--samples` change those positive counts. `--cpu` selects an allowed CPU; otherwise the first allowed CPU is used. The harness and all children are pinned to that CPU. `--timeout` sets the per-process timeout for compilation, probes, warmups, and samples; it defaults to 120 seconds. The independent validator uses its own 120-second process timeout. Generated binaries, LLVM IR, and default JSON output live under the ignored `benchmarks/local/` directory; `--artifact-dir` changes the binary and IR location.

Before timing, the harness compiles once at LLVM `-O3` and runs the differential validator. Validation compares complete output against CPython and checks shortest-path costs against an independent Bellman-Ford implementation on small grids, including large positive and negative seeds. Each measured workload also gets a CPython reference output. Every warmup and measured execution must exit successfully, emit no stderr, and match that entire output. The harness aborts if it is running under a ptrace tracer; run it outside the tracing sandbox so tracing overhead is not mistaken for execution time.

The timer surrounds `subprocess.run` with `time.perf_counter_ns`. Each sample therefore includes fresh process startup, input/output pipes, all route-planner work, and process exit. CPython runs as `python -I examples/route_planner.py`, including interpreter startup and source compilation on every invocation; Relar runs its previously compiled executable. These are end-to-end command-line timings. They do not isolate the algorithm's steady-state execution time. AOT compilation, correctness checks, and warmups are excluded from execution timings. AOT compilation time is reported separately and includes Relar checking/lowering, LLVM optimization, linking, and output creation; it excludes building the Rust compiler and runtime.

Within each pair, execution order alternates between CPython first and Relar first. The JSON stores every raw duration and output, all warmup pairs, input text, source/input/executable/IR/tool hashes, Git revision and dirty status, tool versions, available CPython JIT/GIL/PGO/LTO configuration information, CPU affinity/model, and OS details. PGO/LTO fields describe recorded configure options and compiler flags rather than proving how an interpreter binary was built. Source and tool hashes are checked again after measurement to reject changes during the run. Hostnames and complete Python configure arguments are not collected.

Results report each implementation's median, minimum, maximum, and interquartile range, plus CPython's median divided by Relar's median. Observed spread is not a confidence interval. CPU affinity and alternating order reduce some noise, but a shared devbox still has scheduling interference, frequency changes, and shared cache/memory contention; stop concurrent builds and other experiments before measuring. This is evidence about this route planner, input sizes, and recorded toolchain. It does not establish a general Python speedup, extension compatibility, or a safety guarantee.
