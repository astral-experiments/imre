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
| `argparse` | 1,614 | 1,893 | The intact baseline passed 1,397 cases. Selected runs added 217 distinct original registrations, including 18 more help cases and three Boolean-action, encoding and FileType cases verified with teardown and failing controls. |
| `collections` | 528 | 659 | Eleven new passes came from intact UserDict, UserList, Counter and test_collections runs on a development build. The last of these ran all 116 registrations, with 74 passes at each native level. |
| `csv` | 122 | 128 | Complete suite on `main`, 122 passed and six matched upstream skips. |
| `dataclasses` | 247 | 276 | The latest intact public run passed 206, and the retained intact run passed 207. Thirteen more TestDocString originals pass on a sealed development tree after all 276 register. Their public replay needs annotationlib.get_annotations for #3578. |
| `datetime` | 636 | 1,098 | The intact development build ran all 1,098 registrations under the reference resource settings. Both levels passed 628, errored on 416 and skipped 54. Eight additional originals passed on earlier focused builds. |
| `fnmatch` | 24 | 24 | Complete suite on `main`, all 24 passed. |
| `functools` | 91 | 329 | The intact 325-case primary module passed 89 in all four modes. Two more original cases passed in separate runs. |
| `hashlib` | 67 | 82 | Four earlier complete public runs passed 48/82. Two additional SHAKE originals pass at both levels on public main with the 256 official vectors cached. CPython passes the same inputs, and default-resource runs skip both. |
| `importlib` | 100 | 1,346 | Eighty-four disk-backed importlib.resources originals passed on four public revisions with pinned zipfile source on the extension path. Each case started, passed, restored its fixtures and stopped at both levels. Sixteen earlier originals bring the total to 100. Default discovery still stops before these resource cases. |
| `io` | 112 | 1,038 | The public stateful-decoder original passed in ordinary and boxed O0/O3 and was already among the 112 credited cases. All four builds of the intact 667-case test_io module compiled. Discovery stops at io.BufferedReader. |
| `itertools` | 112 | 138 | The 112 passes come from selected runs. Three original numeric methods and the original batched doctest added four cases, each verified at both levels with a failing control. The last intact public run stopped during import. |
| `json` | 134 | 216 | The public encoder group passes 35 of 36 original cases at O0 and O3. All 35 were already among the 134 recorded passes. TestPyEnum.test_floats still errors. |
| `logging` | 47 | 272 | Fourteen distinct originals passed in selected runs on public c50f49a5, including nine formatting and record cases and five manager and last-resort cases. The optional record case still stops in its real asyncio context and is excluded. |
| `os` | 59 | 579 | Two unchanged classes cover 23 passing DirEntry and scandir cases on public main. Twenty were outside the prior 39, and four of the 23 first passed on this change. The intact OS suite remains incomplete. |
| `pathlib` | 268 | 1,369 | The intact development tree f033d8ae passed 268 of 1,369 originals, with 685 errors and 416 skips at both levels. A matched build reproduced all 233 prior passes. The 35 gains cover 26 ZIP copy, five read and four write cases. Public ZIP byte controls pass, while the original support classes still stop importing typing.runtime_checkable on the public revision tested. |
| `re` | 125 | 165 | Earlier native runs verified 125 distinct originals. On the later public build, the full 165-case run passed 72 with fd 0 closed. The ordinary-stdin run finished one successful case before it stopped. The closed-fd result adds no cases to the table. |
| `shutil` | 61 | 223 | All 15 Linux sendfile originals passed at O0 and O3 on the public d399550c merge using unchanged classes and fixtures. Together with earlier copy and archive cases, 61 originals have passed. The intact suite still stops while importing tarfile. |
| `socket` | 61 | 746 | Earlier and focused cases total 61. Four native modes pass 14/19 resolver originals, including the new service database lookup. The 746-case discovery run executed no test bodies. |
| `subprocess` | 177 | 353 | The intact public run passed 151 of 353 originals. Fourteen other cases passed in earlier runs. Two later public builds passed 12 more cwd-related originals, each with matching startTest, addSuccess and stopTest callbacks. Five selected cases still fail at alternate executable selection. |
| `tempfile` | 84 | 118 | Thirty selected filesystem cases ran in all four public native modes, with 27 passes and three skips. Seven previous errors now pass. Six of those cases had already passed on other builds. |
| `textwrap` | 68 | 68 | Complete suite on `main`, all 68 passed. |
| `threading` | 83 | 228 | All 228 original registrations ran in ordinary and boxed builds at both levels. Each passed 83, skipped 32 and reported failures on 113 IDs. The bound worker callback still blocks 58 Bunch fixture cases. |
| `typing` | 107 | 712 | The public TypedDict class now runs all 32 originals at both levels, with 25 passes, one failure and six erroring IDs. Its overlap with the earlier 107 distinct cases is still being reconciled. |
| `unittest` | 275 | 1,090 | The intact package passed 160 cases. On public #3601, the unchanged test_assertions and test_case files ran all 150 originals with runtime imports. Their 109 paired passes added 81 distinct cases. Another 34 original mock methods passed in focused runs. |
| `urllib` | 145 | 178 | On macOS and Linux, 67/77 parser cases and 75/101 request cases matched the pinned callbacks at O0 and O3. Three historical passes bring the total to 145. A further parser case matched 54 subtest outcomes and one subtest skip without a parent success callback. |
| **Total** | **5,347** | **13,328** | **3 complete suites on `main`** |

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
