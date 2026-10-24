# Independent payout reference

The Rust program implements the [record pipeline](../../../examples/record_pipeline/README.md)
file protocol, parsing checks, arbitrary-precision results and error order. It
first parses `BigInt` records and also retains a contiguous `i64` representation
when every field fits. Queries use checked `i64` subtraction and addition; any
overflow retries the pure query with `BigInt`. The checksum also promotes on
overflow. There is no narrowing of accepted Python integers or repetition counts.

Rust uses ordinary iterator closures that LLVM can inline; Python uses concrete
record objects, runtime-selected callable values and lazy `map`/`filter`.
Both select the same records, transform the same amounts, and compute count,
total and maximum in one traversal. Rust retains the parsed `BigInt` records for
fallback, and passes every repeated query's inputs and result through
`black_box`. This is a useful native implementation comparison, not a claim that
the storage or callback dispatch mechanisms are identical.

Build the reference and prepare the native program separately from timing:

```sh
python3 benchmarks/reference/record_pipeline/build.py \
  --artifact-dir /tmp/record-rust
python3 benchmarks/record_benchmark.py \
  --compiler target/release/imre --runtime target/release/libimre_runtime.a \
  --rust /tmp/record-rust/record-reference --clang clang-18 \
  --prepare-only --artifact-dir /tmp/record-prepared \
  --output /tmp/record-prepared.json
python3 benchmarks/record_benchmark.py \
  --compiler target/release/imre --runtime target/release/libimre_runtime.a \
  --rust /tmp/record-rust/record-reference --clang clang-18 \
  --prepared-record /tmp/record-prepared.json --cpu 0 \
  --artifact-dir /tmp/record-timing --output /tmp/record-timing.json
```

Use a CPU allowed by the host. Preparation compiles at optimization level 3 and
runs all complete-output oracle cases on the exact three engines. The harness
requires committed sources by default; `--allow-dirty` marks exploratory work.
Source, compiler, runtime, interpreter, linker, Rust build-record and executable
hashes are checked before and after measurement. `--imre-build-record` can bind
the compiler/runtime to a separately recorded build.

The four workloads are gross/net transforms with full/selective risk filters,
each at 32 and 4,096 records. Every process parses once, performs enough complete
queries to inspect roughly `--batch-records` records, and writes one full report
and checksum. The default is 1,048,576 records. A separate empty-input control
measures startup/protocol costs; it excludes nonempty parsing and is never
subtracted. Timings include process startup, imports, file reads, parsing,
queries, formatting, report writes/close and exit. Inputs are page-cache warm;
writes are not fsynced. Fixture setup and complete output/filesystem checks occur
outside the timer.

The default is two warmups and twelve measured rounds. Measured rounds repeat
all six engine permutations twice, balancing positions and within-round directed
predecessors. Warmups and cross-round transitions are not balanced. JSON retains
every process result, median and interquartile range. Run timing after builds,
validation and profiling have stopped; the harness rejects ptrace execution.

For matched ThinLTO, build this reference with `--thinlto`, supply a Imre runtime
built with embedded LLVM bitcode, and add `--lto-linker /path/to/matching/ld.lld`
to both preparation and measurement. Both implementations use optimization level
3 and the existing 1,000-instruction import/inline budget; Rust uses 16 codegen
units. Keep ordinary and ThinLTO artifacts in separate fresh directories.
Locally, `build.py --toolchain ohm` selects `cargo +ohm -Zohm-defaults=no`;
CI uses the repository's pinned normal toolchain.
