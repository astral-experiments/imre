# Ruff’s complete conformance reporter

Relar compiles Ruff’s 38,160-byte `scripts/conformance.py` directly. With two real ty releases on the 146-file typing checkout, the native program took 583 ms for the diff report and 588 ms for the GitHub report, both within 3% of Rust. Replaying captured ty output across 1,460 files put the largest difference at 1.26× Rust. [Original Ruff script](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py) · [Measurement record](2026-10-23-original-conformance.json.gz)

| Workload | Format | CPython (ms) | Codon (ms) | Relar (ms) | Rust (ms) | Relar / Rust | Stdout bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 146 files, live ty | diff | 729.5 | 754.4 | 582.7 | 568.8 | 1.02× | 98,898 |
| 146 files, live ty | GitHub | 733.3 | 764.3 | 588.4 | 572.5 | 1.03× | 109,259 |
| 146 files, replay | diff | 247.0 | 257.5 | 94.7 | 83.7 | 1.13× | 99,288 |
| 146 files, replay | GitHub | 257.4 | 267.1 | 96.9 | 83.5 | 1.16× | 109,259 |
| 1,460 files, replay | diff | 1,615.2 | 2,547.5 | 905.4 | 720.5 | 1.26× | 1,002,441 |
| 1,460 files, replay | GitHub | 1,616.5 | 2,509.3 | 896.3 | 735.0 | 1.22× | 1,101,968 |

## What ran

CPython 3.14.5 and Relar ran the same file from a Ruff checkout. Its SHA-256 is `05f1114746f471b365ab0e78edbe12d26724c44767525d2782bcc176c8632772`. Codon 0.20.4 ran the existing native typed reporter with a direct stdout entry point. Its directory iteration preserves CPython’s file order. The independent Rust port used Rust 1.99.0-dev. Both ports parse each analyzer response once, resolve paths for major diagnostics, and write the report and its final newline to stdout. The Relar and Rust builds used the system allocator and ThinLTO. [Codon entry point](../codon_native/conformance_stdout.py) · [Rust entry point](../reference/conformance_report/src/live.rs)

Every run included process startup, TOML parsing, two directory scans, expected-diagnostic collection from source files, two analyzer subprocesses, JSON parsing, path resolution, grouping, rendering, and writing the complete report to a pipe. The live comparisons ran ty 0.0.60 and 0.0.90 from the typing checkout. Replay substitutes two executables that print the captured JSON. Untimed checks compared the working directory and complete ordered argument vector each reporter supplied to both executables. [Typing checkout](https://github.com/python/typing/tree/835b70f3f795f8e061b1f7621178515b0a77a0ad/conformance) · [Benchmark runner](../conformance_program_benchmark.py)

The 146-file replay contains 372,088 bytes of Python and 1,169 old and 1,201 new diagnostics. The larger replay copies each source ten times in total, gives each copy a distinct filename, and updates the diagnostic paths and test groups to match. It contains 3,720,880 source bytes and 23,700 diagnostics. Ruff includes absolute checkout paths in diff output, so the byte count depends on where the fixtures are materialized. The 1× live and replay GitHub reports have the same SHA-256.

The table reports median wall times on an AMD EPYC-Milan Linux host. Replay used one CPU. Live runs used two CPUs with two Rayon workers for ty. After one warmup per engine, each engine ran eight times per case in a balanced order. All 192 measured processes exited successfully, wrote empty stderr, and matched the original CPython stdout byte for byte. The record contains each timing, execution order, report digest, analyzer argument digest, executable identity, and environment. [Raw samples](2026-10-23-original-conformance.json.gz)

## Reproduce

Prepare the existing changed fixture at both scales with `--scales 1 10`. Build the original file with Relar and the two native references with the options below. Set `TYPING` to the pinned checkout and `OLD_TY` and `NEW_TY` to the ty executables. The selected CPU IDs must be available to the runner. Supply a new directory to `--work-dir` to retain the exact checkout paths used for diff output. [Capture preparation](../../examples/conformance_report/README.md#replay-a-captured-comparison) · [ThinLTO toolchain](../../docs/subset.md#build-and-validate) · [Codon native platform library](../codon_native/README.md#live-conformance-workflow)

```sh
"$RELAR" build "$RUFF/scripts/conformance.py" --whole-program --opt-level 3 \
  --runtime "$THIN_RUNTIME" --target x86_64-unknown-linux-gnu \
  --lto-linker "$LTO_LINKER" -o "$BUILD/conformance"
"$CPYTHON" benchmarks/reference/conformance_report/build.py --toolchain ohm --thinlto \
  --artifact-dir "$BUILD/rust"
"$CPYTHON" benchmarks/codon_native/build.py --codon "$CODON" \
  --applications conformance_report --platform-library "$PLATFORM_LIBRARY" \
  --conformance-stdout --output "$BUILD/codon"
"$CPYTHON" benchmarks/conformance_program_benchmark.py \
  --source "$RUFF/scripts/conformance.py" --python "$CPYTHON" --fixtures "$FIXTURES" \
  --checkout "$TYPING/conformance" --relar "$BUILD/conformance" \
  --compiler "$RELAR" --runtime "$THIN_RUNTIME" \
  --codon "$BUILD/codon/conformance_report-stdout" \
  --rust "$BUILD/rust/conformance-report-reference" \
  --cpu 8 --live "$OLD_TY" "$NEW_TY" --live-cpus 20 21 \
  --work-dir "$BUILD/benchmark-checkouts" --output measurements.json.gz
```
