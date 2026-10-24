# Telemetry comparison

Compare a reference recording with an observed recording to detect regressions
in a sensor, simulation, or service metric. This typed Python package runs
unchanged on CPython and Imre. It checks complete timestamp alignment before
answering queries, so an early alert cannot conceal a missing final sample.

```sh
PYTHONPATH=examples python3.14 -m telemetry_comparison report \
  examples/telemetry_comparison/sample/reference.txt \
  examples/telemetry_comparison/sample/observed.txt /tmp/telemetry-report.txt 3 1
cat /tmp/telemetry-report.txt

cargo build --release --workspace --locked
target/release/imre build examples/telemetry_comparison/__main__.py \
  --module-root examples --runtime target/release/libimre_runtime.a \
  --clang clang-18 --target x86_64-unknown-linux-gnu --opt-level 3 \
  -o /tmp/telemetry-comparison
/tmp/telemetry-comparison report \
  examples/telemetry_comparison/sample/reference.txt \
  examples/telemetry_comparison/sample/observed.txt /tmp/telemetry-report.txt 3 1
```

Locally, use `cargo +ohm -Zohm-defaults=no` with the configured target/cache
directories in place of plain Cargo. CI uses the normal pinned toolchain.

The arguments are `MODE REFERENCE OBSERVED REPORT THRESHOLD REPEATS`:

- `report` writes sample count, signed/absolute/squared error totals, whether any
  error exceeds the threshold, whether all are within it, and the latest
  timestamp exceeding it.
- `alert` returns `True` on the first absolute error above the threshold.
- `within` returns `False` on that same first breach.
- `latest` scans from newest to oldest and returns the first breach's timestamp,
  or `None`. Negative timestamps remain distinct from an absent result.

`REPEATS` runs the selected query that many times on the loaded recordings and
writes its final complete result once. Successful stdout is `OK` followed by a
repetition checksum. The report checksum is signed plus absolute plus squared
error; boolean queries contribute zero or one; `latest` contributes its timestamp
or zero. The sample report has five samples, signed error 2, absolute error 12,
squared error 46, and latest breach 1040. Its checksum is 60 per repetition.

## Input and errors

Each UTF-8 file begins with a nonnegative sample count, then exactly that many
`timestamp value` lines. Timestamps must strictly increase within each file and
match between files. Empty recordings are valid: `any` is false, `all` is true,
and `latest` is `None`. A deviation equal to the threshold is within tolerance.

Numeric tokens contain an optional sign and 1–4,300 ASCII decimal digits, without
underscores. Thresholds are nonnegative and repetitions positive. Values,
timestamps, reductions, and checksums retain arbitrary-precision arithmetic.
Decimal rendering follows CPython's default 4,300-digit conversion limit. Python
whitespace and `splitlines()` boundaries are accepted; a final newline is optional
and additional trailing rows are rejected. NUL and invalid UTF-8 OS arguments are
outside the shared command-line domain.

The program parses the reference before opening the observed recording, then
exhausts `zip(..., strict=True)` to validate alignment. Unequal lengths retain
Python's strict-zip diagnostic. After all query and decimal conversions succeed,
it opens, writes, flushes, and closes the report. Input/output aliases and hard
links work because both input contents are captured first. Errors before output
open preserve any existing report; output failures can leave a truncated or
partial report. Writes are neither atomic nor fsynced.

Expected input or I/O failures produce `ERROR`, the exception category, and the
message on stdout, with exit status zero. This is the example's structured result
protocol; unexpected exceptions still fail the process. Stable I/O messages name
the input or report rather than depending on operating-system wording.

## Iterator behavior and validation

Generator expressions feed `sum`, `any`, and `all` directly. Queries use strict
`zip` over typed sample lists; the latest-breach query zips two `reversed`
iterators and stops at the first match. The reductions make separate passes and
do not materialize difference lists. Input validation runs once before repeated
queries, so early-exit timings measure actual short-circuit behavior.

The independent [validator](../../benchmarks/telemetry_validate.py) imports no
application code. Its indexed oracle materializes errors and independently
models file contents, directories, and hard links. It checks complete reports,
stdout, stderr, status, and filesystem effects, including random traces, empty
inputs, threshold boundaries, arbitrary integers, native overflow fallback,
decimal limits, malformed data, alignment errors, aliases, and `/dev/full`.

```sh
python3 benchmarks/reference/telemetry_comparison/build.py \
  --artifact-dir benchmarks/local/telemetry-rust \
  --target-dir target/telemetry-reference
python3 benchmarks/telemetry_validate.py --python python3.14 \
  --binary /tmp/telemetry-comparison \
  --rust benchmarks/local/telemetry-rust/telemetry-reference \
  --output benchmarks/local/telemetry-validation.json
```

See the [independent Rust reference](../../benchmarks/reference/telemetry_comparison/README.md)
for the comparison algorithm and benchmark commands. Functional validation alone
does not establish performance.
