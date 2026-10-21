# Callback payout pipeline

This program turns transaction records into a payout summary. A configured risk
policy selects eligible records. The selected gross or net transform extracts the
amount or subtracts a fee, and a positive-amount callback removes refunds and
zero payouts. The report gives the payout count, total and largest payout.

The same Python source uses `Callable` parameters and return values, a top-level
function value, bound methods that retain locally created policy objects, a
noncapturing lambda, and nested lazy `map`/`filter`, including `filter(None, ...)`.
No intermediate payout list is created.

From the repository root:

```sh
target/release/relar build examples/record_pipeline/__main__.py \
  --module-root examples --clang clang-18 -o /tmp/record-pipeline
/tmp/record-pipeline net examples/record_pipeline/sample/transactions.txt \
  /tmp/payouts.txt 10 25 2
cat /tmp/payouts.txt
```

The executable prints `OK` and checksum `11854`. The report is:

```text
PAYOUTS
COUNT 2
TOTAL 3550
LARGEST 2375
END
```

Run the same package with CPython:

```sh
PYTHONPATH=examples python3 -m record_pipeline net \
  examples/record_pipeline/sample/transactions.txt /tmp/payouts.txt 10 25 2
```

The six arguments are `MODE INPUT REPORT LIMIT FEE REPEATS`. `MODE` is `gross` or
`net`; the nonnegative risk limit and fee are integers. Each input starts with a
record count, followed by that many `ID AMOUNT RISK` rows. IDs must be positive
and strictly increasing; risks are nonnegative. Amounts can be negative. Fields
use signed ASCII decimal tokens of at most 4,300 digits, with Python whitespace
and line splitting. Arithmetic remains arbitrary precision. Decimal output uses
CPython's default 4,300-digit conversion limit.

`REPEATS` must be positive. Input is parsed once, the entire query runs that many
times, and one report is written. The checksum adds `count + total + largest` for
each query. Input and argument failures print `ERROR`, an exception category, and
a normalized message. The report is opened only after successful computation and
formatting; output I/O failures can leave an incomplete report.

Validate complete results, errors and filesystem effects:

```sh
python3 benchmarks/reference/record_pipeline/build.py \
  --artifact-dir /tmp/record-rust
python3 benchmarks/record_validate.py --binary /tmp/record-pipeline \
  --rust /tmp/record-rust/record-reference
```

The independent oracle uses materialized arithmetic over tuples instead of the
callback pipeline. It covers empty and random records, filtering boundaries,
refunds, native-integer overflow and large integers, malformed text, Unicode
separators, output preservation, overwritten inputs, hardlinks and invalid paths.
The [reference guide](../../benchmarks/reference/record_pipeline/README.md)
describes benchmarks and the Rust storage choices.
