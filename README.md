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
branches. Skipped tests count toward the suite size only. The [omission decisions](docs/stdlib-omissions.md) explain each original included in the omitted column.

| Module | Paired passes | Upstream tests | Omitted | How the tests ran |
| --- | ---: | ---: | ---: | --- |
| `argparse` | 1,772 | 1,893 | 0 | Stock CPython and public O0 and O3 passed all 14 originals in the two mutually exclusive group error classes. The selected classes retained their upstream definitions and teardown. Earlier runs account for 1,758 distinct cases, including 14 TestParentParsers originals on a guarded integration. |
| `collections` | 614 | 659 | 6 | The unchanged 295-case OrderedDict file and its 13 classes exited 0 at both levels. Stock passed 295. Imre passed 287 and skipped eight. Six skips have accepted omissions. The two weak-reference cases remain in scope. |
| `csv` | 122 | 128 | 0 | Complete suite on `main`, 122 passed and six matched upstream skips. |
| `dataclasses` | 247 | 276 | 0 | The latest intact public run passed 206, and the retained intact run passed 207. Thirteen more TestDocString originals pass on a sealed development tree after all 276 register. Their public replay needs annotationlib.get_annotations for #3578. |
| `datetime` | 727 | 1,098 | 0 | Both public native builds registered all 1,098 originals and emitted 847 terminal verdicts before aborting. Each passed 591. The 590 that passed both levels contribute 50 new cases to the cumulative 727. |
| `fnmatch` | 24 | 24 | 0 | Complete suite on `main`, all 24 passed. |
| `functools` | 91 | 329 | 0 | The intact 325-case primary module passed 89 in all four modes. Two more original cases passed in separate runs. |
| `hashlib` | 77 | 82 | 0 | The cumulative resource-enabled inventory includes 77 distinct pairs. Under the default stock resource policy, intact ordinary O0 and O3 each record 67 passes, one threaded-hashing error and 14 skips. The real builtin MD5 factory is verified at both native levels. |
| `importlib` | 235 | 1,346 | 0 | With the pinned zipfile extension available, Linux native O0 and O3 passed 125 of 126 disk-resource originals, adding 12 text, bytes and encoding cases. The last requires c_resources. Default staging reached zero of these 126. The ledger also includes 86 runnable originals across three intact importlib.metadata files. |
| `io` | 258 | 1,038 | 0 | The CTextIOWrapperTest class has 60 of 82 originals with paired evidence across public builds. The latest 15 have exit-zero raw unittest results at both optimization levels on unchanged CPython source. The six recent BufferedWriter matches await a public replay. |
| `itertools` | 112 | 138 | 0 | Selected runs account for 112 original passes. Public 90955053 resolved the typing.Iterable import. The unchanged 137-case primary now reaches a module-level type alias in _pyrepl/types.py and stops before discovery at both native levels. A separate GIL companion supplies the 138th registration. |
| `json` | 172 | 216 | 0 | The intact Unicode file passed 34/34 at ordinary O0 and O3 on two public revisions. Of those, 26 used the expected provider and 17 were new. The eight-case scanstring file passed at both levels and added one original. |
| `logging` | 132 | 272 | 0 | All 272 originals registered in stock order on public Linux. All 30 LoggerTest and LoggerAdapterTest originals and all 18 primary filter and level originals passed on stock and public O0 and O3. QueueHandlerTest.test_formatting passed. The QueueListener test errored at Thread.start(). |
| `os` | 230 | 579 | 0 | Public evidence includes eight more Linux file-descriptor originals, among them dup, dup2 and ftruncate. The cumulative set includes earlier EnvironTests and descriptor-inheritance results. |
| `pathlib` | 901 | 1,369 | 0 | Both intact runs exited 1 after all 1,369 originals. The same 865 passed, 404 skipped and 100 errored at O0 and O3. Pinned stock also passed all 865. The 901 cumulative pairs include 36 originals verified in selected runs. |
| `re` | 136 | 165 | 0 | The unchanged 165-case file completed on public ab6da8b9 with live stdin. O0 and O3 each passed 97, errored on 41, failed 13 and skipped 14. Stock passed 162 and skipped three. The intact run adds test_misuse_flags to the previously published 135-case union. [Split and substitution results](benchmarks/cpython_stdlib/full/re/split-subn-linux.tsv) |
| `shutil` | 89 | 223 | 0 | The complete original TestCopyTree, TestMove and TestCopyFile classes passed all 44 Linux-runnable cases on public main, with two matching skips. Sixteen of the passes were already counted. Earlier sendfile and archive results are included. |
| `socket` | 61 | 746 | 0 | Earlier and focused cases total 61. Four native modes pass 14/19 resolver originals, including the new service database lookup. The 746-case discovery run executed no test bodies. |
| `subprocess` | 254 | 353 | 0 | Public Linux O0 and O3 passed six additional originals for kill, send_signal, terminate, waiting with ignored SIGCHLD, and interrupted communicate with and without poll. Their raw receipts show successful parent tests and reaped children. The full 353-case suite remains open. |
| `tempfile` | 107 | 118 | 0 | The intact 118-case file passed 100 originals in ordinary and boxed O0 and O3. Three TemporaryDirectory shutdown cases passed in later runs. All seven TestTemporaryFile cases passed with real files on stock and public ordinary and boxed O0 and O3, adding four more cases. |
| `textwrap` | 68 | 68 | 0 | Complete suite on `main`, all 68 passed. |
| `threading` | 83 | 228 | 0 | All 228 original registrations ran in ordinary and boxed builds at both levels. Each passed 83, skipped 32 and reported failures on 113 IDs. The bound worker callback still blocks 58 Bunch fixture cases. |
| `typing` | 245 | 712 | 0 | All 83 ProtocolTests methods and 132 subtests passed in stock and both public native runs, including deferred annotations. Nineteen other selected typing originals passed at ordinary O0/O3. The cumulative 245 of 712 includes earlier verified originals. |
| `unittest` | 275 | 1,090 | 0 | The intact package passed 160 cases. On public #3601, the unchanged test_assertions and test_case files ran all 150 originals with runtime imports. Their 109 paired passes added 81 distinct cases. Another 34 original mock methods passed in focused runs. |
| `urllib` | 169 | 178 | 0 | Both unchanged scripts exited 0 on public Linux in ordinary and runtime-import O0 and O3. Of 178 originals, 169 passed in the parent, including two verified address normalizations. Three matched in a child only. Two Windows cases and four cases requiring SSL skipped. Ordinary checked compilation still stopped on both files. |
| **Total** | **7,201** | **13,328** | **6** | **3 complete suites on `main`** |

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
