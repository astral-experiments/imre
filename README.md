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
  <img src="benchmarks/results/applications.svg" width="100%" alt="Median elapsed milliseconds for Imre, Rust and CPython on the uv download catalog, a Ruff source search, wheel verification, New York road routing, graph loading and the ty conformance report.">
</p>

Each CLI runs on one AMD EPYC Milan vCPU with warm filesystem caches. Imre and
CPython run the same Python. The catalog includes uv’s original implementation
as a second CPython reference. Road routing includes reading the graph and
finding all four distances.
[Workloads, raw samples and reproduction steps](benchmarks/applications/README.md)

<p align="center">
  <img src="benchmarks/results/frozen-flask.svg" width="100%" alt="Median verified requests per second for frozen Flask on Imre, stock Flask on CPython and Rust/Hyper at eight concurrent clients.">
</p>

The Flask benchmark issues health checks, escaped HTML greetings and JSON sums
over loopback. Each bar is the median of 70 rounds at eight concurrent clients.
Frozen Flask on Imre 0.1.5 compiles its routes and Jinja template
ahead of time. Stock Flask on CPython uses Werkzeug. Rust/Hyper
implements the same routes directly. [HTTP workload and results](benchmarks/applications/README.md#flask)

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

As of November 3, 2026, Imre has a paired pass for **9,470 of 13,328 tests (71.1%)** in the original 25 standard library module families.
A **paired pass** means that CPython 3.14.5 passed the original test and Imre passed it at both
`-O0` and `-O3`. The total combines focused and intact suite runs across revisions, including
development branches. Each original test occurrence counts once. Skipped and omitted tests remain
in the denominator. The [omission decisions](docs/stdlib-omissions.md) explain each original included in the omitted column.

| Module | Paired passes | Upstream tests | Omitted | How the tests ran |
| --- | ---: | ---: | ---: | --- |
| `argparse` | 1,887 | 1,893 | 6 | All 1,887 eligible originals have paired passes across public revisions. The translations test ran the real pygettext child and produced all 32 expected messages. Six explicitly documented compiled-entry omissions remain in the 1,893-test denominator. A complete run on one revision is tracked separately. |
| `collections` | 641 | 659 | 9 | The async-generator ABC original passed on public #4804. All 40 originals in that ABC cohort now have a paired result or an individual omission decision. |
| `csv` | 122 | 128 | 0 | Complete suite on `main`, 122 passed and six matched upstream skips. |
| `dataclasses` | 259 | 276 | 0 | Four deferred-annotation originals passed with the packaged dataclasses and annotationlib modules. Each native run registered all 276 tests and completed teardown. |
| `datetime` | 1,043 | 1,098 | 0 | Four unchanged Fast date and datetime constructor parents passed public #4751 at O0 and O3. Stock passed the matching four in its six-parent group. |
| `fnmatch` | 24 | 24 | 0 | Complete suite on `main`, all 24 passed. |
| `functools` | 93 | 329 | 0 | The intact TestCachePy and TestLRUPy classes ran all 32 original parents through a class driver. Stock and both native levels passed the same 25, including one new parent for LRU hashing. |
| `hashlib` | 77 | 82 | 0 | The cumulative resource-enabled inventory includes 77 distinct pairs. Under the default stock resource policy, intact ordinary O0 and O3 each record 67 passes, one threaded-hashing error and 14 skips. The real builtin MD5 factory is verified at both native levels. |
| `importlib` | 329 | 1,346 | 0 | Seventy-eight more resource tests passed across eight module runs. Sixty-five needed the pinned `zipfile` on the runtime path, and 13 also needed it during compilation. Two originals in `test_files` still errored on Imre. |
| `io` | 699 | 1,038 | 2 | The complete 182-test upstream memoryio module ran on public Imre at both levels. Both native runs passed the same 169 tests, including all 39 pure-Python BytesIO cases. This adds 104 distinct parents to the I/O family. |
| `itertools` | 112 | 138 | 0 | Selected runs account for 112 original passes. Public 90955053 resolved the typing.Iterable import. The unchanged 137-case primary now reaches a module-level type alias in _pyrepl/types.py and stops before discovery at both native levels. A separate GIL companion supplies the 138th registration. |
| `json` | 214 | 216 | 0 | All 34 unmodified CLI originals passed, with 32 child subtests and 64 tool subprocesses per lane. The O0/O3 children ran native code. Two help cases matched the upstream usage-prefix assertion while their full stdout differed from stock. |
| `logging` | 232 | 272 | 0 | Seven more originals passed from the full upstream logging test file in separate stock and native processes. They cover task names, formatter precision, shutdown and recursion. |
| `os` | 428 | 579 | 0 | All three upstream urandom tests passed on public Imre at both optimization levels, adding two previously uncounted parents. |
| `pathlib` | 974 | 1,369 | 1 | Four more `glob` and `rglob` tests passed with recursive symlinks after complete discovery of all 1,369 originals. All three owners exited cleanly, and filesystem observations confirmed the links and teardown in all twelve fixtures. |
| `re` | 147 | 165 | 7 | Seven CPython compiler and bytecode checks have explicit omissions. The 147 paired originals include focused and complete runs. |
| `shutil` | 154 | 223 | 0 | Stock and public native O0/O3 each discovered all 223 originals and ran the same 49 additional tests. All three owners exited 0 with identical parent callbacks and four successful subtests. |
| `socket` | 61 | 746 | 0 | Earlier and focused cases total 61. Four native modes pass 14/19 resolver originals, including the new service database lookup. The 746-case discovery run executed no test bodies. |
| `subprocess` | 300 | 353 | 3 | Seven original credential cases ran in each mode. Three passed on stock and at both native levels, adding two distinct parents. Four cases skipped under the same Linux privilege restrictions. |
| `tempfile` | 110 | 118 | 2 | All three runs registered the same 118 original parents. CPython passed 112 and skipped six. Public Imre passed 110 and skipped eight at both levels. |
| `textwrap` | 68 | 68 | 0 | Complete suite on `main`, all 68 passed. |
| `threading` | 83 | 228 | 1 | All 228 original registrations ran in ordinary and boxed builds at both levels. Each passed 83, skipped 32 and reported failures on 113 IDs. The bound worker callback still blocks 58 Bunch fixture cases. |
| `typing` | 704 | 712 | 1 | The upstream generic subclass-initialization parent and both subtests passed through the public #4855 CLI at O0 and O3 with the same events as stock. |
| `unittest` | 536 | 1,090 | 0 | Both native builds passed 49 of 50 runner tests, 60 of 84 mock helper tests and all six sentinel tests. These results add three distinct parents. |
| `urllib` | 173 | 178 | 0 | Both unchanged upstream scripts completed on public `main` at O0 and O3 with ordinary and runtime imports. All 178 tests matched stock in order, including 173 successful parents, three with only child outcomes, and two Windows skips. |
| **Total** | **9,470** | **13,328** | **32** | **4 complete suites match stock on `main`** |

See the [standard library coverage report](docs/stdlib.md) for the Linux test
results and import coverage recorded in this repository.

### Additional module families

CPython 3.14.5 discovers **1,823 test occurrences** across these 12 families on Linux. They use the same pairing rule and have a separate total.

| Module | Paired passes | Upstream tests | Coverage |
| --- | ---: | ---: | --- |
| `base64` | 12 | 41 | The complete 41-test module ran at both levels. Both public native builds passed the same 12 parents, including six new Base32 cases. |
| `configparser` | 307 | 344 | Six disjoint selections ran 324 of the 344 upstream tests. Both public native builds passed the same 307 parents. Five tests skipped on stock and Imre, and 12 errored on Imre. The denominator includes the remaining 20 tests. |
| `contextlib` | 148 | 149 | All 58 async originals and 90 of 91 synchronous originals have paired passes. The module entry point passed nine of the ten `TestSuppress` cases on public Imre. The remaining case exercises exception groups. |
| `fractions` | 44 | 50 | The complete 50-test module ran on public Imre at both levels. Both native runs passed 44 parents, including three that exercise float components. |
| `glob` | 21 | 24 | Stock CPython registered 24 upstream parents. A selection of 23 ran on all three engines. Each passed 21 and skipped two Windows tests. The separate open-file stress test remains in the denominator. |
| `inspect` | 157 | 362 | Five new parents passed at both levels across annotation formats, wrapped coroutine state and the complete predicate class. Five of the 11 predicate parents passed. |
| `ssl` | 41 | 195 | Three additional SSL context parents passed on public Imre at both levels, including a certificate-store query against OpenSSL. |
| `tokenize` | 49 | 130 | The ordinary unittest entry point ran all 130 original parents on stock and public Imre. Stock passed all 130. O0 and O3 passed the same 49 parents. Errors from individual subtests do not add parent registrations. |
| `tomllib` | 17 | 17 | All 17 originals passed on stock and public O0/O3 through both the package entry point and the three explicit upstream test modules. |
| `types` | 131 | 141 | The method descriptor type original passed through ordinary unittest invocation on stock and public native O0/O3. |
| `warnings` | 181 | 187 | The intact default suite paired 177/187. Four additional async originals passed with `-X context_aware_warnings=1` on pinned stock and public O0 and O3. Each flag-enabled run registered all 187 originals and completed teardown. |
| `weakref` | 169 | 183 | All 46 WeakSet originals and 123 weakref originals passed. Seven [CPython-specific tests](docs/stdlib-omissions.md#weakref) have individual omission decisions. Seven other originals remain unpaired. |
| **Measured total (12 of 12 families)** | **1,277** | **1,823** |  |

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

Imre defaults to native optimization level 3. Set `IMRE_DEFAULT_NATIVE_OPT_LEVEL`
to 0, 1, 2, or 3 when building the compiler to choose a different default.
The compiled-in default also applies to isolated Python invocations (`-I`).

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
