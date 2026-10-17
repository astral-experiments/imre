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

The independent oracle checks 135 inputs, including cycles, forward dependencies, Unicode labels, and arbitrary-precision durations. See the package README for input format and benchmark reproduction.
