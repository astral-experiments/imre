# Rust dependency-planner reference

This package implements the same planner algorithm as
[`examples/dependency_planner`](../../../examples/dependency_planner/README.md)
using ordinary Rust structs, vectors, and borrowed references. Its manifest,
lockfile, and Rust source are unchanged from the original local Rust comparison.
It is a reference for this program, with an explicit input contract.

The source retains adjacency construction, the handwritten min-heap, duplicate
edges, validation precedence, minimum-ID ties, sink selection, reverse parent
traversal, first-match name lookup, and the complete report. Each path-string
concatenation constructs a fresh string. Input consumes one UTF-8 line at a time,
removes LF, and splits with Python whitespace, including U+001C through U+001F.
Numeric input follows Relar's ASCII decimal format, including optional signs and
underscores between digits. Python's other Unicode decimal digits are outside
this format.

Two builds use the same source:

- `bigint` uses `num-bigint = 0.4.8` and `num-traits = 0.2.19` for durations,
  dependency values, starts, finishes, and makespan. Invalid dependencies remain
  arbitrary precision until reported.
- `checked-i64` uses signed 64-bit values for those fields. Out-of-range input or
  an overflowing finish exits with `OverflowError`; arithmetic never wraps.

Task IDs, indegrees, heap positions, and vector lengths use `usize`; conversions
from input dependencies are checked. An unaddressable task count is an overflow
error. Output uses an 8 KiB buffer for pipes/files, terminal newline flushing,
and a final flush. Graph errors produce the same successful four-line report.
Other input, overflow, and returned I/O failures emit a category and message on
stderr and exit 1. Python tracebacks, signals, invalid UTF-8 handling, and
recoverable allocation failure are outside the reference's compatibility scope.

Build the two references from the repository root:

```sh
python3 benchmarks/reference/dependency_planner/build.py
```

The default artifacts are `benchmarks/local/dependency-reference/planner-bigint`,
`planner-i64`, and `build-record.json`; Cargo's default target directory is a
`target` subdirectory there. `--artifact-dir`, `--target-dir` (or
`CARGO_TARGET_DIR`), `--build-cache` (or `CARGO_BUILD_BUILD_DIR`), and `CARGO_HOME`
allow separate local paths. `--offline` uses cached dependencies. The checked-in
lockfile is required and never regenerated. The record captures both binaries,
source hashes and complete source text, commands, versions, and build durations.
Rust flags are cleared to avoid introducing machine-specific optimization.

Public and CI builds use normal Cargo. On this devbox, pass `--toolchain ohm`;
the helper uses `cargo +ohm -Zohm-defaults=no` and `rustc +ohm`, with a separate
target directory and an optional shared build cache. The manifest uses the
normal release optimization profile, generic CPU settings, and line-table debug
information. No PGO or LTO is requested.

The five-way comparison uses a saved baseline Relar executable and its benchmark
record, the current compiler/runtime, unchanged CPython source, and both Rust
binaries. To create a baseline, run `benchmarks/dependency_benchmark.py` at the
desired baseline revision with `--sizes 128 2048 16384 --shapes layered wide`,
preserving its executable and JSON record. After building the desired optimized
compiler/runtime, run:

```sh
python3 benchmarks/dependency_compare.py \
  --baseline benchmarks/local/baseline/dependency_planner \
  --baseline-record benchmarks/local/baseline.json \
  --compiler target/release/relar \
  --runtime target/release/librelar_runtime.a \
  --rust-bigint benchmarks/local/dependency-reference/planner-bigint \
  --rust-i64 benchmarks/local/dependency-reference/planner-i64 \
  --rust-build-record benchmarks/local/dependency-reference/build-record.json \
  --python python3.14 --samples 30 --warmups 2 \
  --output benchmarks/local/dependency-comparison.json
```

The harness compiles optimized Relar once at LLVM `-O3` before validation and
timing, saving its executable and IR under `benchmarks/local` by default. Build
time is recorded separately; building the compiler/runtime itself is excluded.
An existing Rust pair can be reused without rebuilding: pass its binary paths,
original build record, and `--rust-source-root` pointing to the original source
directory containing `build.py`. Its Rust source must still match this tracked
reference byte for byte. The old binaries and source records remain untouched.

Before timing, every implementation must match the independent planner oracle:
135 cases for CPython, both Relar binaries, and Rust BigInt; 121 in-range cases for
checked i64. The 14 excluded cases and reasons are retained. Ten additional
boundary cases check signed limits, accumulated overflow, schedulable components
of cyclic graphs, graph-error precedence, signs/underscores, and embedded NUL
labels. Four cases require explicit checked-i64 overflow rejection. The oracle
and boundary checks also run independently, without benchmarking or a saved
baseline:

```sh
python3 benchmarks/reference/dependency_planner/check_boundaries.py \
  --relar benchmarks/local/dependency-planner-optimized/dependency_planner \
  --rust-bigint benchmarks/local/dependency-reference/planner-bigint \
  --rust-i64 benchmarks/local/dependency-reference/planner-i64 \
  --python python3.14
```

An optional `--baseline` adds the saved executable to those correctness checks.
The standalone helper records case counts, excluded numeric inputs, complete
boundary output evidence, and executable/source hashes, checking for mutation
before writing its JSON result.

The comparison reuses the baseline record's complete six inputs and expected
outputs: layered and wide DAGs with 128, 2,048, and 16,384 tasks. Every warmup and
sample starts a fresh process, receives identical bytes, and must return identical
complete output, empty stderr, and exit status zero. Full input and expected
output are stored once per workload; each run retains its duration, status,
stderr, and output hash/length. All source, tool, binary, harness, and input-record
hashes are checked again after measurement. The baseline record must identify the
exact saved binary and every unchanged Python source file.

The ten Williams orders are cyclic shifts of `[0, 1, 4, 2, 3]` and their reversals.
In each cycle every implementation occupies each position twice, and each
directed adjacent pair within a round occurs twice. The default 30 rounds repeat
that cycle three times; sample counts must be multiples of ten. The complete
order cycle and every raw round are recorded. Predecessors across round boundaries
are not balanced.

Timing surrounds `subprocess.run`: startup, input/output pipes, parsing,
scheduling, rendering, and process exit are included. CPython runs the unchanged
package through the isolated bootstrap; ordinary imported-module bytecode caching
is allowed. Builds, validation, warmups, output comparisons, and hashing are
excluded. Timing requires Linux: the harness refuses ptrace and pins itself and
children to the first allowed CPU, or `--cpu`. Run with other project work idle. Shared-host contention
and CPU frequency remain uncontrolled; medians, quartiles, and observed ranges
describe these samples without establishing general language speed limits.

This is a direct Rust port with different layouts and ownership operations.
BigInt preserves the relevant arbitrary-precision fields; checked i64 has a
narrower numerical contract. Small workloads include substantial process and I/O
overhead. Compiler/backend versions are recorded and may differ between Rust and
Relar. No C implementation is measured.
