# Benchmarks

The example computes shortest routes across weighted grids using Dijkstra's algorithm and its own binary heap. CPython and Relar execute the same `examples/route_planner.py` source, receive the same input, and must produce exactly the same output. The compiler generates LLVM IR and links a native executable against the independent Rust runtime. There is no rewritten native kernel or CPython interpreter inside that executable.

The [first recorded result](results/2026-10-17-route-planner.md) includes raw samples and environment details: 2.17× faster than CPython 3.14.5 for three routes on 256 × 256 grids, using end-to-end process timings.

The [profiling follow-up](results/2026-10-17-route-planner-optimized.md) records the native bottlenecks and their optimization: 2.60× faster than the preserved original native executable and 5.66× faster than CPython in a direct three-way comparison, with the Python source unchanged.

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

## Comparing native changes

Preserve the baseline executable before rebuilding. `compare.py` runs that executable, an optimized executable, and unchanged CPython in the same session:

```sh
python3 benchmarks/compare.py \
  --baseline benchmarks/local/profile-baseline/route_planner \
  --optimized benchmarks/local/artifacts/route_planner \
  --baseline-record benchmarks/results/2026-10-17-route-planner.json \
  --optimized-record benchmarks/local/route-planner.json \
  --python python3.14 \
  --output benchmarks/local/comparison.json
```

The optional build records must match the current source and executable hashes. The comparison does not rebuild either binary. It uses the same correctness oracle, timing boundaries, CPU affinity, and tracing guard as `benchmark.py`. Defaults are two warmup triplets and twelve measured triplets, covering all six execution orders twice. `--samples` accepts other positive counts; a multiple of six keeps every implementation equally represented in each execution position. The JSON records each order and raw duration, hashes, native build provenance when supplied, and both optimized-to-baseline and optimized-to-CPython ratios.

## Profiling the executable

Callgrind can collect instruction and call counts without hardware-counter access. Profile the same prebuilt executables and inputs, keeping instrumented profiling separate from normal timing:

```sh
printf '256\n3\n42\n' > benchmarks/local/profile-input.txt
valgrind --tool=callgrind --dump-instr=yes --collect-jumps=yes \
  --callgrind-out-file=benchmarks/local/callgrind.out \
  benchmarks/local/artifacts/route_planner < benchmarks/local/profile-input.txt
callgrind_annotate --auto=no --inclusive=no --threshold=100 benchmarks/local/callgrind.out
```

Callgrind reports executed instructions, not CPU cycles or elapsed-time shares. Rust inline source locations may split one function across several rows; aggregate them by function when comparing totals. LLVM may inline generated Python functions into their callers, and identical runtime functions can share a symbol address: `relar_list_retain` and `relar_str_retain` currently do so. Inspect symbols and callers before attributing that shared entry to strings.

## Dependency planner

The [dependency-planner package](../examples/dependency_planner/README.md) exercises static imports, typed classes, general lists, tuples, and optional values in a complete scheduling program. Its independent oracle and `dependency_benchmark.py` compare the unchanged package on CPython and Relar, using the same fresh-process timing, single-CPU affinity, and tracing guard. See the package README for input semantics, correctness checks, reproduction commands, and the compact per-sample output records.

The [recorded comparison and I/O profile](results/2026-10-17-dependency-planner.md) include six workloads and a preserved native baseline. At 16,384 tasks, buffered Relar runs the layered graph 2.39× faster than CPython and 10.97× faster than the original native build. The three-way harness rotates all six execution orders and verifies every complete report.

The [CPU and allocation follow-up](results/2026-10-17-dependency-performance.md) compares optimized Relar with CPython and algorithm-matched Rust references. At 16,384 tasks, optional ThinLTO makes Relar about 1.5× faster than the buffered baseline and 3.69×/5.26× faster than CPython on layered/wide graphs. It approximately matches arbitrary-precision Rust on the layered graph and remains 46% slower on the wide graph, with LLVM optimization budgets matched. The report includes ordinary-link controls, cumulative allocation profiles, correctness checks, and raw samples.

The [range-proof and container-access follow-up](results/2026-10-17-dependency-index-specialization.md) reduces large wide-graph time by another 20%, narrowing the matched Rust BigInt gap from 46% to 16%. Relar is 4.04×/6.67× faster than CPython on large layered/wide graphs with ThinLTO. The report preserves ordinary-link controls, 30-sample raw comparisons, cumulative CPU/allocation profiles, and validation of bounds, ownership, aliases, and arbitrary-precision fallbacks. The Python package and Rust reference algorithm remain unchanged.

The five-way `dependency_compare.py` comparison and native correctness suites accept optional `--lto-linker /path/to/ld.lld --target x86_64-unknown-linux-gnu` flags. Supply a compatible runtime archive; these harnesses do not rebuild it. The comparison records the requested and effective target, build flags, and linker version and hash. Linker symlink names are preserved because they select LLD's flavor. Omitting these flags retains the default linking behavior.

`dependency_profile.py` profiles preserved executables using the inputs and complete expected outputs from a comparison record. For example, after producing `benchmarks/local/dependency-comparison.json` and its native artifact:

```sh
python3 benchmarks/dependency_profile.py \
  --record benchmarks/local/dependency-comparison.json \
  --binary optimized=benchmarks/local/dependency-comparison/dependency_planner \
  --sizes 16384 --shapes layered wide \
  --output benchmarks/local/dependency-profile
```

This runs Callgrind and DHAT separately, requires a new output directory, and verifies exit status, complete output, and executable hashes. Supply `--build-record optimized=/path/to/build-record.json` to preserve additional build provenance. Instrumented runs are excluded from timing results; the output includes trace/log hashes and instruction/allocation summaries. Valgrind is discovered on `PATH` or in the Relar tool cache, or can be selected with `--valgrind` and `--valgrind-lib`.

## Incremental planner

The [named incremental planner](../examples/incremental_planner/README.md) uses dictionaries, sets, and direct iteration to validate a named graph and schedule the targets affected by changes. The unchanged package runs on CPython and Relar; [Rust references](reference/incremental_planner/README.md) implement the same algorithm with either arbitrary-precision or checked-i64 arithmetic.

Build and validate all four implementations before measuring:

```sh
target/release/relar build examples/incremental_planner/__main__.py \
  --module-root examples --runtime target/release/librelar_runtime.a \
  --clang clang-18 --opt-level 3 -o /tmp/incremental-planner
python3 benchmarks/reference/incremental_planner/build.py \
  --artifact-dir benchmarks/local/incremental-reference-normal \
  --target-dir target/incremental-reference
python3 benchmarks/incremental_validate.py --binary /tmp/incremental-planner \
  --rust-bigint benchmarks/local/incremental-reference-normal/planner-bigint \
  --rust-i64 benchmarks/local/incremental-reference-normal/planner-i64 \
  --python python3.14
```

Validation checks 155 graph cases and 17 separate parser cases, including complete output and fatal error categories. Checked i64 matches 138 graph cases and must explicitly reject the other 17 for overflow; all parser cases are in range. The reference guide gives ordinary and ThinLTO build commands, local Ohm options, and `incremental_benchmark.py` reproduction commands. Its four-engine comparisons use 32 measured rounds with balanced Williams orders, two warmups, one pinned CPU, fresh processes, exact output checks, and preserved source/tool/build provenance. Default workloads cover sparse and full changes in 16,384-target layered and wide graphs plus a smaller integer-boundary case. These are whole-process measurements, including parsing, whole-graph validation, scheduling, and the complete report.
