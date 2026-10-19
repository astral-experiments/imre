# Relar

An ahead-of-time compiler for typed Python, written in Rust.

> [!WARNING]
> This project's code, PR summaries, and documentation were authored by AI agents.
> Relar is experimental and is not yet a production-ready replacement for CPython.
> Use at your own risk.

## Highlights

- Compile first-party Python programs and packages into native executables.
- Use static imports, typed classes, containers, comprehensions, slicing, keyword arguments, and scalar f-strings.
- Use binary64 floats, immutable bytes, structural pattern matching, and assignment expressions.
- Use Python truth tests, operand-returning `and`/`or`, and `for`/`while` `else` suites.
- Preserve arbitrary-precision integers, shared mutation, and supported exception behavior.
- Run compiled programs without a Python installation or a CPython dependency.

| Workload | Positive requests | Relar | CPython 3.14.5 | Speedup |
| --- | ---: | ---: | ---: | ---: |
| Dense inventory reconciliation | 16,384 | 21.24 ms | 76.70 ms | 3.61× |
| Sparse inventory reconciliation | 16,384 | 40.88 ms | 112.38 ms | 2.75× |

These [Linux measurements](benchmarks/results/2026-10-19-truthiness-loop-else.md) run
the same inventory program under Relar and CPython, with ThinLTO for the native
builds. Times are medians of 48 fresh processes per workload and include startup,
file parsing, reconciliation, and complete report writing, flushing, and closing.
Inputs are created before timing on a shared machine. Relar takes 4.1–8.7% more
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
Relar rejects unsupported syntax and type violations during compilation. Functions
require parameter and return annotations; local types can be inferred.

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

## License

Relar is licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Relar
by you, as defined in the Apache-2.0 license, shall be dually licensed as above, without any
additional terms or conditions.
