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

See the [benchmark method and reproduction commands](benchmarks/README.md) and [recorded results](benchmarks/results/2026-10-17-route-planner.md). The first run measured a 2.17× median end-to-end speedup over CPython 3.14.5 for three routes on 256 × 256 grids.
