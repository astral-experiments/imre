# Independent Rust telemetry reference

This implementation uses the same file protocol, query passes, arbitrary-precision
results, diagnostics, and repetition checksum as the
[Python example](../../../examples/telemetry_comparison/README.md). It links no
Relar runtime code. Inputs are parsed into `BigInt` records. If timestamps and
values fit `i64` and the threshold fits `i128`, queries use contiguous native
records and checked `i128` reductions; overflow restarts that query with `BigInt`.
The checksum promotes from `i128` to `BigInt` on overflow. Large inputs therefore
retain the same arithmetic rather than being excluded from validation.

The report makes the same three reduction passes followed by `any`, `all`, and a
reverse search. Each repeated call and its result pass through `black_box`.
The final result is rendered once. Native Rust records and iterator loops have
different layouts and ownership costs from Python's typed objects and generated
loops; the comparison is between complete equivalent programs.

The ASCII-decimal input grammar and default 4,300-digit decimal-output limit are
explicit. Python whitespace and line boundaries are reproduced independently.
Both inputs close before output opens, and close failures are checked explicitly.
The validator compares exact errors and complete filesystem effects, including
hard-link aliases and output-device failure.

```sh
python3 benchmarks/reference/telemetry_comparison/build.py \
  --artifact-dir benchmarks/local/telemetry-rust \
  --target-dir target/telemetry-reference
```

Local builds add `--toolchain ohm`; `--build-cache` selects a shared build
directory distinct from `--target-dir`. The helper clears inherited Rust flags,
sets release optimization level 3 with 16 codegen units, and records complete
sources, tool identities, flags, build output, and executable hash. `--thinlto`
uses ThinLTO with import/inlining budgets of 1,000. Normal and ThinLTO builds need
separate artifact directories.

## Benchmark

```sh
python3 benchmarks/telemetry_benchmark.py \
  --compiler "$TOOLS/relar" --runtime "$TOOLS/librelar_runtime.a" \
  --relar-build-record "$TOOLS/build-record.json" \
  --rust "$RUST/telemetry-reference" --rust-build-record "$RUST/build-record.json" \
  --python python3.14 --clang clang-18 --target x86_64-unknown-linux-gnu \
  --prepare-only --artifact-dir benchmarks/local/telemetry-prepared \
  --output benchmarks/local/telemetry-prepared.json

python3 benchmarks/telemetry_benchmark.py \
  --compiler "$TOOLS/relar" --runtime "$TOOLS/librelar_runtime.a" \
  --relar-build-record "$TOOLS/build-record.json" \
  --rust "$RUST/telemetry-reference" --rust-build-record "$RUST/build-record.json" \
  --python python3.14 --clang clang-18 --target x86_64-unknown-linux-gnu \
  --prepared-record benchmarks/local/telemetry-prepared.json \
  --cpu 0 --artifact-dir benchmarks/local/telemetry-normal \
  --output benchmarks/local/telemetry-normal.json
```

Preparation compiles the Python package and validates the exact Relar/Rust
executables against the independent oracle and CPython. Timing rechecks every
recorded source, tool, binary, LLVM, and validation hash before and after running.
For ThinLTO, select the corresponding Rust build and Relar runtime and add
`--lto-linker /path/to/ld.lld` to both commands. Matching modes are required.
The compile command always passes an explicit target, defaulting to the Rust
host when `--target` is omitted. Relar's build target must match that host, and the
produced executable's ELF class, byte order, and machine must match the Rust
executable. Relar's emitted LLVM text has no target directive; it is retained and
hashed without treating it as target evidence.

Defaults are 16, 256, and 4,096 samples, with report, full-scan predicates/search,
and early-exit predicate/search workloads. Repetition counts target 1,048,576
source-level inspected sample pairs per process; early-exit cases therefore make
more calls than full scans. Every process includes input loading/alignment once.
Eight warmup rounds precede 12 measured rounds that cover all six engine orders
twice. Measured positions and within-round directed predecessors are balanced;
warmups and cross-round transitions are not. All raw samples are retained.

Timing includes startup, input parsing/alignment, repeated queries, output,
flush/close, and exit. Fixture preparation and complete output/filesystem checks
are outside the timer. Empty-recording controls report startup/protocol cost
without subtraction. CPU affinity is pinned, inputs use warm OS caches, and
writes are not fsynced. Run outside ptrace after other builds, tests, and profilers
stop. `--batch-pairs`, `--sizes`, and `--scenarios` control exploratory runs;
published comparisons must state the selected values.
