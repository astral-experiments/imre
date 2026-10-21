# Unchanged Luhn

This example compiles [`mmcloughlin/luhn` 0.2.0](https://github.com/mmcloughlin/luhn/tree/d6f3fa71072f99334a4c1d2a3f062fa93982797f).
`luhn.py`, `upstream/test.py`, and `upstream/LICENSE` are byte-for-byte upstream copies.
[UPSTREAM.json](UPSTREAM.json) pins the annotated tag, commit, Git blob identities,
sizes, and SHA-256 hashes.

The upstream functions have no annotations. The separate typed driver supplies
string call sites for `checksum`, `verify`, `generate`, and `append`. Compilation
uses the original `list(map(int, string))`, stepped slices, list comprehension,
`sum`, and `divmod` expressions.

The [recorded comparison](../../benchmarks/results/2026-10-20-luhn.md) passes the
upstream tests and 1,144 CPython differential cases in each of five native
configurations. ThinLTO Relar runs 1.60–1.69× faster than CPython but takes
28.9–51.4× the streaming Rust reference's time. The report includes raw timings,
allocation and instruction profiles, validation evidence, and reproduction
metadata.

From the repository root, after building Relar:

```sh
target/release/relar build examples/luhn_checksum/driver.py \
  --module-root examples/luhn_checksum \
  --runtime target/release/librelar_runtime.a --clang clang-18 \
  --opt-level 3 -o /tmp/luhn
printf '7992739871' > /tmp/luhn-input
/tmp/luhn append /tmp/luhn-input /tmp/luhn-output 1
cat /tmp/luhn-output
```

The output file contains `79927398713`. The command accepts an operation, input
file, output file, and positive repetition count. It strictly decodes UTF-8 once,
calls the unchanged library on every repetition, and writes the final complete
result once. Scalar results use their Python string spelling. Standard output is
`OK` followed by an accumulated checksum: the numeric result for `checksum` and
`generate`, 0/1 for `verify`, or output character count for `append`.

Failures print the exception category, exact message, suppression flag, and
explicit cause, and preserve any existing output file. Leading/trailing spaces
and newlines are inputs to the library; the driver does not strip them.

```sh
python3 tests/luhn.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --clang clang-18 --python python3.14
```

The suite compiles at `-O0` and `-O3`, runs the 10 unchanged upstream tests and 5
doctest examples against CPython and native adapters, and checks empty inputs,
random and long numbers, every decimal digit recognized by the selected CPython
Unicode database, mixed digit families, invalid-character ordering, malformed
UTF-8, exact errors, and output-file effects. Separate native checks exercise
repeated exception recovery and imports. `--sanitize`, `--target`, `--lto-linker`,
and `--opt-levels` select the other supported build configurations.

This validates the library's string API. It does not claim support for every
arbitrary Python iterable accepted by the unannotated upstream functions, or
provide a Python-callable extension module.

The [independent Rust reference](../../benchmarks/reference/luhn_checksum/README.md)
uses the same protocol and checksums. To benchmark all four APIs:

```sh
python3 benchmarks/reference/luhn_checksum/build.py \
  --artifact-dir benchmarks/local/luhn-reference \
  --target-dir benchmarks/local/luhn-reference-target
python3 benchmarks/luhn_benchmark.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --clang clang-18 --python python3.14 \
  --rust benchmarks/local/luhn-reference/luhn-reference \
  --artifact-dir benchmarks/local/luhn-normal \
  --output benchmarks/local/luhn-normal.json
```

Defaults use 16, 256, and 4,096 digits per call, 16 MiB of digits per process,
six warmup rounds, and twelve measured rounds. Every engine order is balanced;
each run verifies the complete output and accumulated checksum. Whole-process
times include startup, imports where applicable, file I/O, and the repeated
calls. An empty-input control reports startup/protocol overhead without
subtracting it. Increase `--batch-digits` when that control is a significant
fraction of a sample; `--native-only` measures Relar/Rust while retaining CPython
validation.

Use committed sources, a fresh artifact directory, and an idle untraced CPU.
The harness records CPU affinity, raw samples, commands, inputs, output hashes,
CPython build settings, source/tool/binary hashes, LLVM output, and the Rust build
record. `--relar-build-record` additionally verifies a compiler/runtime build
record containing `sha256` and `source_sha256` identities. For ThinLTO, build a
separate Rust reference with `--thinlto`, supply the matching Relar LTO runtime,
and pass `--target` and `--lto-linker /path/to/ld.lld` to the benchmark.
