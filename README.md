# Imre

An experimental ahead-of-time compiler for typed Python, written in Rust.

> [!WARNING]
> This README includes human edits; all code changes, PR summaries, and additional
> documentation were authored entirely by GPT-6 Astra in [Codex](https://openai.com/codex/).
>
> Imre implements only a small part of Python and its standard library,
> with no guarantee of future development or maintenance. Use at your own risk.

## Benchmarks

<p align="center">
  <img src="benchmarks/results/runtime-suite.svg" width="100%" alt="Six panels comparing median elapsed time for Rust, Imre and CPython on inventory reconciliation, Base45 decoding, Luhn checksums, telemetry and offline conformance reporting.">
</p>

Imre and CPython 3.14.5 run the same Python programs. The Rust versions use
the same inputs and produce the same outputs. Each bar shows the median time
for 12 fresh processes pinned to one AMD EPYC Milan CPU.

<p align="center">
  <img src="benchmarks/results/frozen-flask.svg" width="100%" alt="Mixed HTTP throughput for a frozen Flask app on Imre and equivalent Node.js, Bun and Rust/Hyper handlers, in requests per second.">
</p>

The frozen Flask app compiles its routes and Jinja template ahead of time.
Node.js, Bun and Rust/Hyper implement the same routes directly.

## Hello, world!

```python
# hello.py
def greet(name: str) -> str:
    return f"Hello, {name}!"


print(greet("world"))
```

On Apple Silicon macOS or 64-bit Linux, build and run the executable.

```console
$ imre build hello.py
$ ./hello
Hello, world!
```

Or run directly, as native code:

```console
$ imre hello.py
Hello, world!
$ imre -I -B -c 'import sys; print(sys.implementation.name)'
imre
$ uv python find imre
/path/to/bin/imre
$ uv run --python imre hello.py
Hello, world!
```

## Language support

Imre compiles a restricted subset of Python into native executables that run
without a Python installation. This program prints the names of everyone who
scored at least eight points, in alphabetical order.

```python
from dataclasses import dataclass


@dataclass
class Result:
    name: str
    score: int


def finalists(results: list[Result], cutoff: int) -> list[str]:
    return sorted(result.name for result in results if result.score >= cutoff)


results = [Result("Mira", 8), Result("Jo", 4), Result("Ada", 10)]
print(", ".join(finalists(results, 8)))  # Ada, Mira
```

Imre can resolve `result.name` and `result.score` at compile time. If you add
the loop below, CPython prints `Mira` and `8`. Imre rejects this use of
`getattr` because the loop chooses the field by name at runtime.

```python
for field in ["name", "score"]:
    print(getattr(results[0], field))
```

At present, Imre doesn't support any of the following:

- Dynamically typed storage (`Any`) and unrestricted unions
- Dynamic attributes and general-purpose reflection
- Recursive calls and async generators
- Most of the Python standard library (see [module coverage](docs/stdlib.md))

At the same time, Imre is capable of running scripts like
[ty’s conformance report](examples/conformance_report/README.md) unchanged,
with Rust-competitive performance.

## Standard library test results

The table covers 25 standard library modules as of November 2, 2026 (UTC).
A **paired pass** means the same original CPython 3.14.5 test passed at both
`-O0` and `-O3`. Counts include selected tests and results from development
branches. Skipped tests count toward the suite size only.

| Module | Paired passes | Upstream tests | How the tests ran |
| --- | ---: | ---: | --- |
| `argparse` | 1,397 | 1,893 | Selected cases across three builds, with intact discovery of all 1,893 cases |
| `collections` | 481 | 659 | Selected cases, registered doctests, and an intact 295-case OrderedDict run |
| `csv` | 122 | 128 | Complete suite on `main`, six skips |
| `dataclasses` | 220 | 276 | Selected cases |
| `datetime` | 314 | 1,098 | The first 543 cases produced verdicts before the intact run timed out |
| `fnmatch` | 24 | 24 | Complete suite on `main` |
| `functools` | 41 | 329 | Selected cases across two development builds |
| `hashlib` | 65 | 82 | Selected and intact runs (45 passed together on `main`) |
| `importlib` | 15 | 1,346 | 15 of 16 selected cases in both import modes |
| `io` | 110 | 1,038 | 110 of 123 selected cases across five test suites |
| `itertools` | 107 | 138 | Selected cases on one development tree |
| `json` | 109 | 216 | Selected cases across builds (103 passed together) |
| `logging` | 25 | 272 | Selected cases |
| `os` | 38 | 579 | 25 intact genericpath cases and 13 selected OS cases whose module teardown errored |
| `pathlib` | 225 | 1,369 | 220 in the intact development run and five more in a 139-case selection |
| `re` | 120 | 165 | 120 passed in an intact development run, 69 on a smaller set of changes |
| `shutil` | 34 | 223 | Selected cases |
| `socket` | 47 | 746 | Intact suite on a development branch |
| `subprocess` | 33 | 353 | Selected cases |
| `tempfile` | 5 | 118 | Selected cases |
| `textwrap` | 68 | 68 | Complete suite on `main` |
| `threading` | 67 | 228 | All 228 cases ran on a development branch |
| `typing` | 107 | 712 | Selected tests across development builds |
| `unittest` | 160 | 1,090 | Intact package runner discovered 244 cases and started 213 |
| `urllib` | 84 | 178 | Selected cases |
| **Total** | **4,018** | **13,328** | **3 complete suites on `main`** |

See the [standard library coverage report](docs/stdlib.md) for the Linux test
results and import coverage recorded in this repository.

## Design

Types are part of Imre's compilation contract: variables, containers, and class
fields have known types, and each function has one declared or inferred
signature. This lets Imre choose concrete representations. It aims to preserve
Python's behavior for supported operations, including arbitrary-precision integers.

Imre's runtime uses reference counting and restricts object graphs to avoid
cycles. Compiled programs run without a Python installation. Their imports must
use supported Python or native extensions rebuilt against Imre’s `Python.h`.

Other projects make different trade-offs:

- [PyPy](https://pypy.org/) optimizes dynamic Python with a JIT as the program
  runs. It supports much more of Python, but needs time to warm up.
- [Codon](https://docs.exaloop.io/) also compiles Python ahead of time. It can
  specialize functions for different argument types and can call CPython
  explicitly. Its default `int` is 64-bit.
- [Static Python](https://facebookincubator.github.io/cinderx/static-python) and
  [mypyc](https://mypyc.readthedocs.io/) use types to optimize code running
  within CPython, so compiled modules can interact with ordinary Python code.

Use [`@imre.struct`](docs/subset.md#immutable-value-structs) for a record
whose fields are read-only `float` or `bool` values. Imre can store each
point’s `x` and `y` coordinates directly in a `list[Point]`, without allocating
a separate object for each point. Replacing a list entry leaves the original
point intact.

```python
import imre


@imre.struct
class Point:
    x: float
    y: float


points: list[Point] = [Point(1.0, 2.0)]
original = points[0]
points[0] = Point(3.0, 4.0)
print(original.x)  # 1.0
print(points[0].x)  # 3.0
```

For more, see the [design](docs/design.md) and [supported subset](docs/subset.md).

## Installation

Install Imre on Linux or Apple Silicon macOS with the standalone installer.

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://labs.astral.sh/imre/install.sh | sh
```

The x86-64 and ARM64 Linux releases include Clang 18, LLD 18, and the native
link inputs that `imre build` and `imre run` need.

To build from source, use the [Rust toolchain](rust-toolchain.toml).

```console
git clone https://github.com/viarius-experiments/imre.git
cd imre
cargo build --workspace --release --locked
./target/release/imre --version
```

## Getting started

Given `totals.py`:

```python
from sys import argv


def total(quantities: list[int]) -> int:
    result = 0
    for quantity in quantities:
        result += quantity
    return result


print(f"Total: {total([int(value) for value in argv[1:]])}")
```

Compile and run it in one command.

```console
imre run totals.py -- 10 20 12
# Output: Total: 42
```

Use `imre check --watch totals.py` to see diagnostics as you edit the script or its imports.

Use `imre build` to create a native executable in the current directory. You can then run `./totals` directly.

```console
imre build totals.py
./totals 10 20 12
# Output: Total: 42
```

## License

Imre is licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Vendored example code retains its original license; the Base45 codec uses
[BSD-2-Clause](examples/base45_codec/upstream/LICENSE), and the Luhn checksum
library uses its [upstream license](examples/luhn_checksum/upstream/LICENSE).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Imre
by you, as defined in the Apache-2.0 license, shall be dually licensed as above, without any
additional terms or conditions.
