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

The [batch planner](examples/batch_planner/README.md) exercises catchable builtin exceptions and `finally` cleanup. It captures each request's complete frame, reports malformed payloads, and continues with later requests in the same process. Its graph algorithm and reports come from the unchanged incremental planner. An independent oracle compares CPython, Relar, and Rust BigInt on 194 inputs. The [recorded comparison](benchmarks/results/2026-10-18-batch-planner.md) measures 2.31–3.94× faster execution than CPython on large valid/mixed batches with ThinLTO, taking 1.70–1.82× Rust BigInt's time. The unchanged single-request control records 8.4–22.7% overhead from the compiler/runtime changes with ThinLTO. A separate recovery stress test holds peak live requested heap at 18,140 bytes as caught failures increase from 800 to 80,000; this is an allocation measurement, not process RSS.

The [exception-performance follow-up](benchmarks/results/2026-10-18-exception-performance.md) reduces large batch times by 8.4–14.5% with ThinLTO while preserving the same source and behavior. Those workloads run 2.57–4.67× faster than CPython and take 1.55–1.57× Rust's time. The unchanged programs retain 3.8–6.8% overhead versus the pre-exception compiler.

The [file-manifest planner](examples/manifest_planner/README.md) reads requests from UTF-8 files, recovers from malformed or missing payloads, and writes a complete report. It exercises `sys.argv`, sealed `typing.TextIO`, builtin-file `with` cleanup, scalar string conversion, and `splitlines`, using the existing parser and graph algorithm. The package includes a runnable sample and an independent oracle that checks complete filesystem effects against CPython and Rust. The [recorded comparison](benchmarks/results/2026-10-18-manifest-planner.md) measures 1.86–3.27× faster execution than CPython on large file workloads with ThinLTO, taking 1.72–1.79× Rust BigInt's time. Unchanged-program medians move by less than 1% in both linking modes.

The [concrete-layout follow-up](benchmarks/results/2026-10-18-concrete-layouts.md) removes optional wrapper allocations, gives classes and tuples concrete LLVM layouts, and specializes container storage. Unchanged large incremental, batch, and manifest workloads take 11.5–16.4% less time with ThinLTO, running 2.20–5.24× faster than CPython and taking 1.32–1.55× Rust BigInt's time. The large layered planner allocates 37.7% fewer bytes and has 25.2% lower peak live requested heap.

The [inventory reconciler](examples/inventory_reconciliation/README.md) exercises eager typed list, dictionary, and set comprehensions, including nested iteration, ordered filters, and helpers that mutate shared inventory. It reads stock and batched requests from UTF-8 files, fulfills positive requests in order, and writes complete fulfillment, shortage, demand, and remaining-stock reports. Its independent oracle checks full filesystem effects against CPython and a Rust BigInt reference, including duplicate SKUs, huge quantities, malformed inputs, output failures, and input/output aliases.
