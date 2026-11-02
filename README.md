# <img src="docs/assets/imre-mark.svg" width="24" height="24" alt=""> Imre

An experimental ahead-of-time compiler for typed Python, written in Rust.

> [!WARNING]
> This README includes human edits; all code changes, PR summaries, and additional
> documentation were authored entirely by GPT-6 Astra in [Codex](https://openai.com/codex/).
>
> Imre implements only a small part of Python and its standard library,
> with no guarantee of future development or maintenance. Use at your own risk.

## Benchmarks

<p align="center">
  <img src="benchmarks/results/runtime-suite.svg" width="100%" alt="Rust has the lowest median time on all six workloads. CPython takes 1.7 to 12.2 times as long as Imre. Each workload has its own time scale.">
</p>

Imre and CPython 3.14.5 run the same Python programs. The Rust versions use
the same inputs and produce the same outputs. Each bar shows the median time
for 12 fresh processes pinned to one AMD EPYC Milan CPU.

<p align="center">
  <img src="benchmarks/results/frozen-flask.svg" width="100%" alt="Median mixed HTTP throughput in requests per second. Frozen Flask on Imre handles 7,802, Node.js 2,266, Bun 7,733 and Rust/Hyper 8,138.">
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
| `argparse` | 1,593 | 1,893 | An intact run passed 1,397 originals. Selected runs added 196 distinct registrations across builds, including 54 more help cases with clean teardown and inverted assertions. |
| `collections` | 528 | 659 | Eleven new passes came from intact UserDict, UserList, Counter and test_collections runs on a development build. The last of these ran all 116 registrations, with 74 passes at each native level. |
| `csv` | 122 | 128 | Complete suite on `main`, 122 passed and six matched upstream skips. |
| `dataclasses` | 234 | 276 | An intact development suite passed 207 of 276. Other distinct cases passed on separate builds. |
| `datetime` | 553 | 1,098 | Both intact development runs passed 545 of 1,098. Eight additional original cases passed in focused numeric and warning tests. |
| `fnmatch` | 24 | 24 | Complete suite on `main`, all 24 passed. |
| `functools` | 91 | 329 | The intact 325-case primary module passed 89 in all four modes. Two more original cases passed in separate runs. |
| `hashlib` | 65 | 82 | Four complete public runs passed 48 of 82, with three errors and 31 skips. Earlier selections account for the other credited IDs. |
| `importlib` | 16 | 1,346 | Sixteen originals have verified native fixtures. Public metadata changes passed 29 original method bodies on CPython-created disk fixtures. Those results are tracked separately while the native fixture runs are incomplete. |
| `io` | 112 | 1,038 | The public stateful-decoder original passed in ordinary and boxed O0/O3 and was already among the 112 credited cases. All four builds of the intact 667-case test_io module compiled. Discovery stops at io.BufferedReader. |
| `itertools` | 108 | 138 | Source-preserving selections passed across builds, including the CPU-enabled long-chain test. The intact module stops before registration. |
| `json` | 134 | 216 | The public encoder passed 33 of 36 selected originals at both levels. Three earlier encoder failures already had passing evidence elsewhere. The list-recursion case adds one to the campaign total. Intact package discovery still stops at sys.monitoring. |
| `logging` | 30 | 272 | Thirty source-isolated originals ran together with native fixtures. Package-wide discovery stopped during import. |
| `os` | 39 | 579 | The public stat_result pickle original passed at both levels, including protocols 0–5. It adds one to 38 earlier OS and genericpath cases. Full test_os discovery remains incomplete. |
| `pathlib` | 233 | 1,369 | Both intact development runs finished all 1,369 cases and passed the same 233. |
| `re` | 125 | 165 | Four complete public runs passed 64 of 165. All 330 individual O0 and O3 runs agreed. Earlier builds account for the remaining distinct IDs. |
| `shutil` | 46 | 223 | Public native runs matched CPython on a 17-case copy selection with 16 passes and one platform skip. Hard-link self-copy and named-pipe copying added two cases. The full module still stops importing tarfile. |
| `socket` | 60 | 746 | Four private intact runs registered 746 and passed 19 methods in every mode. Focused and earlier runs account for the other distinct IDs. |
| `subprocess` | 47 | 353 | Public main imported the unchanged module and registered all 353 IDs in order. Sixteen originals ran directly from that module and matched reference streams at both levels. Four extend the earlier 43-case total. |
| `tempfile` | 83 | 118 | Four intact public runs passed the same 83 of 118, with 24 errored IDs, one failure, 10 skips and zero ignored finalizer exceptions. |
| `textwrap` | 68 | 68 | Complete suite on `main`, all 68 passed. |
| `threading` | 81 | 228 | All 228 original cases ran in four native modes. An intact development build passed 81 and a later replay passed 77. |
| `typing` | 107 | 712 | The complete public TypedDict class ran all 32 originals at both levels. Nineteen passed, one failed, and 12 errored. The table retains 107 until those 19 can be reconciled with the older inventory. |
| `unittest` | 160 | 1,090 | An intact package run discovered 244 originals, started 213 and passed 160 at both levels. |
| `urllib` | 97 | 178 | The intact public parser matched CPython on 51 of 77 cases across eight native configurations. Its 101-case companion stopped during import. |
| **Total** | **4,756** | **13,328** | **3 complete suites on `main`** |

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
