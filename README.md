# Relar

An ahead-of-time compiler for typed Python, written in Rust.

> [!WARNING]
> This project's code, PR summaries, and documentation were authored by AI agents.
> Relar is experimental and is not yet a production-ready replacement for CPython.
> Use at your own risk.

## Highlights

- Compile typed Python programs and selected pure-Python packages into native executables.
- Use static imports, typed classes, containers, comprehensions, slicing, keyword arguments, and scalar f-strings.
- Use binary64 floats, immutable bytes, structural pattern matching, and assignment expressions.
- Use Python truth tests, operand-returning `and`/`or`, and `for`/`while` `else` suites.
- Chain assignments, unpack sequences and starred targets, and expand typed lists, sets, and dictionaries.
- Infer concrete function signatures and consume concrete iterables with `list`, integer `sum`, and `map(int, ...)`.
- Compose generator expressions, `zip(strict=True)`, and `reversed`, with short-circuiting `any` and `all`.
- Pass typed function values and bound methods, use noncapturing lambdas, and compose lazy `map`/`filter` pipelines.
- Preserve arbitrary-precision integers, shared mutation, and supported exception behavior.
- Run compiled programs without a Python installation or a CPython dependency.

| Workload | Positive requests | Relar | CPython 3.14.5 | Speedup |
| --- | ---: | ---: | ---: | ---: |
| Dense inventory reconciliation | 16,384 | 21.09 ms | 76.64 ms | 3.63× |
| Sparse inventory reconciliation | 16,384 | 41.58 ms | 112.60 ms | 2.71× |

These [Linux measurements](benchmarks/results/2026-10-19-assignment-unpacking-expansion.md) run
the same inventory program under Relar and CPython, with ThinLTO for the native
builds. Times are medians of 48 fresh processes per workload and include startup,
file parsing, reconciliation, and complete report writing, flushing, and closing.
Inputs are created before timing on a shared machine. Relar takes 1.9–9.7% more
time than the Rust BigInt reference; the [benchmark guide](benchmarks/README.md)
describes the method, comparisons, and reproduction commands.

Relar owns the compiler and runtime; Ruff provides parsing and ty provides Python
semantic analysis. An additional checker enforces exact types and a restricted
language contract before emitting LLVM IR. Validation combines Rust tests,
differential checks against CPython, sanitizer checks, and independent application
oracles; the [supported-subset guide](docs/subset.md) describes remaining gaps.

## Installation

Build from this checkout on 64-bit Linux with the
[pinned Rust toolchain](rust-toolchain.toml) and Clang 18:

```console
cargo build --workspace --release --locked
```

This builds `target/release/relar` and `target/release/librelar_runtime.a`. Keep them
together when compiling programs, or select the runtime with `--runtime`.
Commands below run from the repository root with Cargo's default target directory.
CPython 3.12 or later is needed only to run the Python examples and differential
tests. See the [build guide](docs/subset.md#build-and-validate) for validation,
sanitizers, and optional cross-language ThinLTO.

## Getting started

Given `totals.py`:

```python
def total(quantities: list[int]) -> int:
    result = 0
    for quantity in quantities:
        result += quantity
    return result


print(f"Total: {total([10, 20, 12])}")
```

Check, compile, and run the program:

```console
target/release/relar check totals.py
target/release/relar build totals.py --clang clang-18 -o /tmp/totals
/tmp/totals
```

The executable prints `Total: 42`. The same source runs with `python3 totals.py`.
Relar rejects unsupported syntax and type violations during compilation. Function
types can come from annotations or be inferred from concrete direct calls,
scalar defaults, and checked return expressions. Each function has one exact
signature; conflicting callers require an explicit supported common type.

To compile and run the inventory reconciler, including its imported modules:

```console
target/release/relar build examples/inventory_reconciliation/__main__.py \
  --module-root examples --clang clang-18 -o /tmp/inventory-reconciliation
/tmp/inventory-reconciliation \
  examples/inventory_reconciliation/sample/stock.txt \
  examples/inventory_reconciliation/sample/orders.txt /tmp/inventory-report.txt
cat /tmp/inventory-report.txt
```

See the [inventory guide](examples/inventory_reconciliation/README.md) for input
formats, report fields, and validation. The [supported-subset guide](docs/subset.md)
covers syntax and compatibility, the [design](docs/design.md) explains compilation
and ownership, and the [benchmark guide](benchmarks/README.md) collects the example
programs, performance results, and reproduction instructions. Run
`target/release/relar --help` for compiler options.

The [python-base45 example](examples/base45_codec/README.md) compiles an unchanged
upstream codec and compares it with CPython, including its original tests, error
causes, and shared module lookup table. Its guide covers building, file-based use,
the enforced type boundary, and benchmark reproduction.

The [luhn example](examples/luhn_checksum/README.md) compiles the unchanged,
unannotated checksum library. Its guide covers the original upstream tests,
Unicode decimal inputs, and comparisons with CPython and an independent Rust
implementation.

The [telemetry example](examples/telemetry_comparison/README.md) aligns two sample
recordings, calculates integer error metrics, and searches for threshold
violations with lazy forward and reverse iteration.

The [record pipeline](examples/record_pipeline/README.md) selects eligible
transactions with configured callbacks, transforms gross or net payouts, and
aggregates a report through lazy `map` and `filter`.

## License

Relar is licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Vendored example code retains its original license; the Base45 codec uses
[BSD-2-Clause](examples/base45_codec/upstream/LICENSE), and the Luhn checksum
library uses its [upstream license](examples/luhn_checksum/upstream/LICENSE).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Relar
by you, as defined in the Apache-2.0 license, shall be dually licensed as above, without any
additional terms or conditions.
