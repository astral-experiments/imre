# Independent memory report reference

This Rust program implements the [typed two-file memory report](../../../examples/memory_report/README.md).
Its ordinary path deserializes directly into contiguous records containing native
`i64` fields. Addition and subtraction are checked. If input or computation
exceeds that representation, it retries from retained input text with
arbitrary-precision integers. The normal path does not allocate a `BigInt` or
carry a large integer union for each item. Both paths use stable sorting and the
same report renderer; only displayed values cross into the general integer
formatting helper. Large integer division is rounded directly from the exact
rational to binary64, including ties and subnormals.

Serde decodes explicit record schemas with required fields, scalar/list defaults,
known-field duplicate rejection, ignored unknown values, and the same 128-level
depth limit. The arbitrary-integer path borrows raw JSON number tokens, rejecting
fractional/exponent tokens and objects impersonating private number wrappers.
Input is read once per file and retained only for a possible fallback. Each
process executes the actual CLI once and writes its full report, so there is no
repeated loop for the compiler to eliminate.

Prepare separately from timing:

```sh
python3 benchmarks/reference/memory_report/build.py \
  --artifact-dir /tmp/memory-rust
python3 benchmarks/memory_benchmark.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --rust /tmp/memory-rust/memory-report-reference --clang clang-18 \
  --prepare-only --artifact-dir /tmp/memory-prepared \
  --output /tmp/memory-prepared.json
python3 benchmarks/memory_benchmark.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --rust /tmp/memory-rust/memory-report-reference --clang clang-18 \
  --prepared-record /tmp/memory-prepared.json --cpu 0 \
  --artifact-dir /tmp/memory-timing --output /tmp/memory-timing.json
```

Use an allowed CPU, and stop other builds, profiles, and timing jobs first.
Preparation compiles at optimization level 3 and validates the exact executables.
The harness requires committed sources; `--allow-dirty` labels exploration.
Compiler, runtime, interpreter, linker, program sources, captures, helpers,
reference build record, LLVM, and executable hashes are checked before and after
measurement. `--relar-build-record` optionally binds a separately recorded build.

Four engines are measured separately: unchanged upstream CPython, the typed
CPython port including its schema-checking shim, Relar, and Rust. The two genuine
capture pairs remain unmodified. Default `--copies 32 256` also produces 8,128
and 65,024 items per file by duplicating the Ruff capture with distinct names and
multiplying totals. Each size includes a full old-to-new comparison and an
unchanged old-to-old comparison that stops after decoding and checking totals.
These larger files are explicitly synthetic extensions of real captures.

Every measured invocation includes startup, imports, both complete file reads,
JSON decoding, comparison, sorting, formatting, output write/close, and exit.
The upstream script additionally echoes its report to captured stderr in file
mode; that output is also checked. All engines' complete output and filesystem
effects are verified outside timing. Upstream tie choices are checked for valid
equal-magnitude ranking and pinned with `PYTHONHASHSEED=0`; the two ports use
deterministic first-appearance ties. Inputs become page-cache warm, and output is
not fsynced.

The default is two warmup rounds and twelve measured rounds. Four Williams
orders balance each engine's position and each directed within-round neighbor;
cross-round transitions and warmups are not balanced. The JSON records all raw
process results, medians, interquartile ranges, and complete output hashes.
Empty schema-valid reports provide a startup/JSON/file-I/O control, without
subtraction; this control omits nonempty parsing and comparison costs. Judge the
large-case Rust ratio together with that control fraction.

For matched ThinLTO, build Rust with `--thinlto`, use a Relar runtime with embedded
LLVM bitcode, and pass `--lto-linker /path/to/matching/ld.lld` to preparation and
measurement. Both use optimization level 3 and the existing 1,000-instruction
import/inline budget; Rust uses 16 codegen units. Keep each mode's artifacts in a
fresh directory. Local `build.py --toolchain ohm` uses
`cargo +ohm -Zohm-defaults=no`; CI uses the repository's normal pinned toolchain.
