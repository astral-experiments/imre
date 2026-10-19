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

The suite verifies the upstream hashes, builds at `-O0` and `-O3`, runs all five unchanged upstream test methods against CPython and a native adapter, and compares 269 additional cases with CPython. It checks exact results, error messages, explicit cause metadata, and file effects. Separate native checks cover every single-byte input, shared lookup-table mutation through import aliases, mixed text/bytes union storage, and recovery across repeated failures.

Two upstream tests deliberately pass the integer `42` to annotated APIs. Relar rejects those calls during compilation. The adapter requires that rejection before satisfying the upstream `TypeError` assertion; these are reported separately as two compile-time boundary tests, not native runtime `TypeError` tests. This does not provide a dynamically typed Python-callable extension API.

The suite also accepts `--sanitize`, `--target`, `--lto-linker`, and `--opt-levels` for the matching runtime archive. AddressSanitizer runs enable leak detection. Explicit cause chains are immutable and may be attached only to freshly constructed builtin exceptions, which keeps the reference-counted heap acyclic.

## Benchmark

Commit measured sources before running:

```sh
python3 benchmarks/base45_benchmark.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --clang clang-18 --python python3.14 \
  --artifact-dir benchmarks/local/base45-normal \
  --output benchmarks/local/base45-normal.json
```

The harness compares encode, bytes decode, and text decode at 64, 1,024, and 16,384 decoded bytes per call. Each process represents about 2 MiB of raw data through repeated API calls; decoding consumes the corresponding 3 MiB of Base45 data. The exact measured binary passes the full compatibility validator before timing. It pins both implementations to one CPU, alternates their order over 20 measured rounds, and records all durations, output hashes, source/tool identities, CPython build settings, and the generated executable and LLVM hashes. Timings include process startup, imports, one input read, codec calls, and one output write; fixture preparation and result verification are outside the clock. Run without tracing and with other builds and profiling stopped.

For ThinLTO, supply a runtime built with `-Clinker-plugin-lto` together with `--target` and `--lto-linker /path/to/ld.lld`. Use a fresh artifact directory for each comparison. `--allow-dirty` permits exploratory runs and records that state; published measurements use committed sources.
