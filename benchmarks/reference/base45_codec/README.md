# Independent Rust Base45 reference

This dependency-free reference implements the same `encode`, `decode-bytes`, and
`decode-text` file protocol as `examples/base45_codec/driver.py`. It reads once,
repeats the codec operation and checksum, and writes the final result once. Failed
transformations leave the output file unchanged. `black_box` surrounds each call's
input and result so LLVM cannot discard repeated work.

The codec uses preallocated `Vec<u8>` output and a fixed alphabet lookup table.
Lookup and output production share a pass; range failures are deferred until all
alphabet checks finish, preserving the upstream decoder's error priority. Text
mode decodes UTF-8 once and trims trailing newline
characters on each call; bytes mode decodes UTF-8 on each call and keeps newlines.
The comparison driver never mutates the upstream module's exported alphabet map.
Rust borrowing and native storage have different allocation costs from Python.

The small diagnostic adapter matches category, explicit cause, suppression, and
messages over the declared validation corpus. It does not implement universal
CPython Unicode `repr`: Rust quotes some combining marks differently and uses its
own Unicode version. Those explicitly recorded cases still check error metadata
and unchanged output. Imre is checked against CPython's exact messages for every
case. Timed Base45 input contains only valid ASCII alphabet characters, so this
diagnostic boundary does not affect performance comparisons. Positive repetition
counts must fit `usize`; checksums use `u128`, sufficient for addressable inputs and
repetition counts. Invalid CLI arguments and operating-system error text are
outside the shared codec corpus.

Build with the repository's selected toolchain, preserving source and tool hashes:

```sh
python3 benchmarks/reference/base45_codec/build.py \
  --artifact-dir benchmarks/local/base45-reference \
  --target-dir benchmarks/local/base45-reference-target
```

For local Ohm builds, add `--toolchain ohm`. An optional `--cargo-wrapper` accepts
a local POSIX-shell Cargo wrapper, and `--build-cache` selects a shared build
directory distinct from the target directory. Neither is required.

The outputs are `base45-reference` and `build-record.json`. The helper records
exact sources, compiler versions, flags, optional wrapper contents, and binary
hash. Inherited Rust flags are cleared; `CARGO_PROFILE_RELEASE_*` overrides and
optional `--llvm-budget` are recorded. Use a separate artifact/target directory for
a ThinLTO build with `CARGO_PROFILE_RELEASE_LTO=thin`. When comparing against
Imre's expanded ThinLTO import and inlining budgets, pass `--llvm-budget 1000` to
give the Rust reference the same budgets.

```sh
python3.14 benchmarks/base45_validate.py \
  --rust benchmarks/local/base45-reference/base45-reference \
  --output benchmarks/local/base45-reference/validation.json
```

Pass this executable as `--rust` to `benchmarks/base45_benchmark.py`. The build
record defaults to the file beside it. The default comparison balances all six
Imre/CPython/Rust execution orders over 24 samples and six warmups. `--native-only`
compares Imre and Rust with alternating order while retaining CPython validation;
larger `--batch-bytes` values reduce the proportion spent on process startup.
Use `--operations encode` for an encoder-only comparison, or select any combination
of `encode`, `decode-bytes`, and `decode-text`. The default measures all three
operations at each of the three payload sizes; selected operations and batch size
are recorded in the result.
