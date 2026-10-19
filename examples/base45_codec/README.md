# Unchanged python-base45

This example compiles the upstream [`python-base45`](https://github.com/kirei/python-base45) codec at commit [`5f49a519c3884a4fbc4c58c5c8f1fa25d7046020`](https://github.com/kirei/python-base45/tree/5f49a519c3884a4fbc4c58c5c8f1fa25d7046020). `base45/__init__.py` and `base45/py.typed` are byte-for-byte copies. The upstream tests and BSD-2-Clause license are retained under `upstream/`; [UPSTREAM.json](UPSTREAM.json) records every original path and SHA-256 hash.

The codec's existing annotations are sufficient. Relar compiles its module lookup table, `typing.Union[bytes, str]`, `isinstance` branches, integer bit operations, `divmod`, `list.extend`, `str.rstrip`, and explicit exception causes. No source rewrite, CPython embedding, or interpreted fallback is involved. The separate upstream `argparse` command-line application is outside this example.

## Build and use

From the repository root:

```sh
cargo build --workspace --release --locked
target/release/relar build examples/base45_codec/driver.py \
  --module-root examples/base45_codec \
  --runtime target/release/librelar_runtime.a --clang clang-18 \
  --opt-level 3 -o /tmp/base45
printf 'AB' > /tmp/base45-input
/tmp/base45 encode /tmp/base45-input /tmp/base45-encoded 1
cat /tmp/base45-encoded
/tmp/base45 decode-bytes /tmp/base45-encoded /tmp/base45-decoded 1
cmp /tmp/base45-input /tmp/base45-decoded
```

The encoded contents are `BB8`. The driver accepts `encode`, `decode-bytes`, or `decode-text`, followed by an input file, output file, and positive repetition count. Successful calls write the full result and print `OK` plus a checksum. Decoding errors print their category, message, suppression flag, and explicit cause, leaving an existing output file untouched. `decode-text` first decodes the input file as strict UTF-8; the upstream codec strips trailing newlines from text input but not bytes input.

The repetition count supports batched measurements. Each repetition calls the original codec on the same input and contributes to the checksum; input is read once and the last complete result is written once.

## Compatibility checks

```sh
python3 tests/base45.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --clang clang-18 --python python3.14
```

The suite verifies the upstream hashes and builds at `-O0` and `-O3`. At each level, it runs all five unchanged upstream test methods against CPython and a native adapter, checking 323 validly typed native calls and 289 further CPython differential cases. Results, error messages, explicit cause metadata, and file effects must agree exactly. Separate native checks cover every single-byte input, shared lookup-table mutation through import aliases, mixed text/bytes union storage, and recovery across repeated failures.

Two upstream tests deliberately pass the integer `42` to annotated APIs. Relar rejects those calls during compilation. The adapter requires that rejection before satisfying the upstream `TypeError` assertion; these are reported separately as two compile-time boundary tests, not native runtime `TypeError` tests. This does not provide a dynamically typed Python-callable extension API.

The suite also accepts `--sanitize`, `--target`, `--lto-linker`, and `--opt-levels` for the matching runtime archive. AddressSanitizer runs enable leak detection. Explicit cause chains are immutable and may be attached only to freshly constructed builtin exceptions, which keeps the reference-counted heap acyclic.

## Benchmark

The [initial comparison](../../benchmarks/results/2026-10-19-base45.md) preserves historical ordinary and ThinLTO results, raw samples, validation, and allocation profiles. Those measurements predate the owned string append optimization. Relar now reuses uniquely owned string storage with geometric capacity growth for eligible appends, avoiding repeated whole-prefix copies in this codec. Existing aliases retain their previous contents. The upstream Python source remains unchanged.

The [independent Rust reference](../../benchmarks/reference/base45_codec/README.md) implements the same typed file protocol with a fixed alphabet and preallocated byte buffers. It performs the same repeated calls and checksum, using `black_box` to prevent elimination of repeated work. Its build helper uses the repository's selected Cargo/Rust toolchain by default and records source, compiler, flags, and executable hashes; optional Ohm configuration is documented in the reference guide.

Commit measured sources, build the reference, then run the three-engine comparison:

```sh
python3 benchmarks/reference/base45_codec/build.py \
  --artifact-dir benchmarks/local/base45-reference \
  --target-dir benchmarks/local/base45-reference-target
python3 benchmarks/base45_benchmark.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --clang clang-18 --python python3.14 \
  --rust benchmarks/local/base45-reference/base45-reference \
  --artifact-dir benchmarks/local/base45-normal \
  --output benchmarks/local/base45-normal.json
```

The harness compares encode, bytes decode, and text decode at 64, 1,024, and 16,384 decoded bytes per call. By default, each process represents 2 MiB of raw data through repeated API calls; decoding consumes the corresponding 3 MiB of Base45 data. All three engines are pinned to one CPU. Six warmup rounds cover every execution order once, and 24 measured rounds cover every order four times. Omitting `--rust` retains a two-engine Relar/CPython comparison with alternating order. Custom `--samples` and `--warmups` must balance all execution orders.

The exact measured executables pass validation before timing. Relar passes the upstream and CPython checks above. The Rust validator runs 295 checks, including four declared Unicode `repr` message boundaries: those four compare error category, cause, suppression, and unchanged output, while permitting different character quoting. Relar matches CPython's exact messages in every case. The reference guide describes this diagnostic boundary; timed decoder inputs use valid ASCII Base45 and do not exercise it. The harness reads `build-record.json` beside the Rust executable unless `--rust-build-record` selects another record.

For sustained encoder comparisons, increase the repeated work and time only the native engines:

```sh
python3 benchmarks/base45_benchmark.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --clang clang-18 --python python3.14 \
  --rust benchmarks/local/base45-reference/base45-reference \
  --native-only --operations encode --batch-bytes 67108864 \
  --artifact-dir benchmarks/local/base45-native-encode \
  --output benchmarks/local/base45-native-encode.json
```

This handles 64 MiB of raw data per process at each payload size, alternates Relar and Rust execution order, and retains CPython validation. `--operations` can select any combination of the three codec operations. Larger batches reduce the proportion of process startup in each sample; measurements still include startup, imports where applicable, one input read, codec calls, checksum, and one output write. Fixture preparation and result verification are outside the clock. Every duration, output hash, source/tool identity, CPython build setting, and generated executable/LLVM hash is recorded. Run without tracing and with other builds and profiling stopped.

For ThinLTO, supply a runtime built with `-Clinker-plugin-lto` together with `--target` and `--lto-linker /path/to/ld.lld`. Build a separate Rust reference with `CARGO_PROFILE_RELEASE_LTO=thin`; the reference guide documents matching LLVM optimization budgets. Use separate artifact and target directories for ordinary and ThinLTO builds, and a fresh artifact directory for each comparison. `--allow-dirty` permits exploratory runs and records that state; published measurements use committed sources.
