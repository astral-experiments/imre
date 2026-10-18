# Relar

Experiments in ahead-of-time compilation of a typed Python subset with LLVM and an independent runtime.

The compiler and runtime are written in Rust. Ruff provides parsing and the AST; ty provides semantic analysis and diagnostics. Relar enforces a stricter type contract and emits LLVM IR with explicit reference ownership.

See the [supported subset and build instructions](docs/subset.md).

The [initial design](docs/design.md) defines enforced types, deliberately restricted dynamic behavior, explicit ownership, and a path to supporting rebuilt extensions later. The first experiment compiles our own weighted-grid route planner and compares the unchanged Python source against CPython.

With the pinned Rust toolchain and Clang 18 installed:

```sh
cargo build --workspace --release --locked
target/release/relar build examples/route_planner.py -o /tmp/route-planner
printf '128\n3\n42\n' | /tmp/route-planner
printf '128\n3\n42\n' | python3 examples/route_planner.py
```

See the [benchmark method and reproduction commands](benchmarks/README.md), [initial results](benchmarks/results/2026-10-17-route-planner.md), and [profiling follow-up](benchmarks/results/2026-10-17-route-planner-optimized.md). The optimized compiler runs three routes on 256 × 256 grids 2.60× faster than the original native build and 5.66× faster than CPython 3.14.5, using the unchanged Python source and median end-to-end process timings.

The [dependency planner](examples/dependency_planner/README.md) exercises static package imports, typed classes and methods, nested lists, fixed tuples, and optional values. It computes a deterministic task schedule and critical path, validates dependency errors, and prints a complete report. The same six source files run under CPython and compile into one native executable:

```sh
target/release/relar build examples/dependency_planner/__main__.py \
  --module-root examples -o /tmp/dependency-planner
python3 benchmarks/dependency_validate.py --binary /tmp/dependency-planner
```

The independent oracle checks 135 inputs, including cycles, forward dependencies, Unicode labels, and arbitrary-precision durations. At 16,384 tasks, the [range-proof and container-access comparison](benchmarks/results/2026-10-17-dependency-index-specialization.md) records 4.04× faster execution than CPython for layered graphs and 6.67× for independent tasks, including the complete report. With matched ThinLTO settings, Relar takes 11% less time than the arbitrary-precision Rust reference on the layered graph and 16% more on the wide graph. The [earlier profile](benchmarks/results/2026-10-17-dependency-planner.md), [initial baseline](benchmarks/results/2026-10-17-dependency-planner-baseline.md), and raw samples are preserved; the package README describes input format and reproduction.

The [incremental planner](examples/incremental_planner/README.md) adds named targets, dictionary lookup, sets, and direct iteration. It validates the whole graph, finds everything affected by a set of changed targets, and emits a deterministic rebuild schedule:

```sh
target/release/relar build examples/incremental_planner/__main__.py \
  --module-root examples --runtime target/release/librelar_runtime.a \
  --clang clang-18 -o /tmp/incremental-planner
python3 benchmarks/incremental_validate.py --binary /tmp/incremental-planner
```

Its independent checks cover 155 graph cases and 17 parser cases. The [recorded four-engine comparison](benchmarks/results/2026-10-17-incremental-planner.md) measures 2.28–4.67× faster execution than CPython on 16,384-target workloads with matched ThinLTO; Relar takes 1.49–1.57× as long as Rust BigInt. The [Rust references and benchmark guide](benchmarks/reference/incremental_planner/README.md) preserve the same algorithm and distinguish arbitrary-precision arithmetic from checked-i64 limits.

The [batch planner](examples/batch_planner/README.md) exercises catchable builtin exceptions and `finally` cleanup. It captures each request's complete frame, reports malformed payloads, and continues with later requests in the same process. Its graph algorithm and reports come from the unchanged incremental planner. An independent oracle compares CPython, Relar, and Rust BigInt on 194 inputs; the benchmark separately measures regression on the unchanged single-request program and throughput of valid and mixed batches.
