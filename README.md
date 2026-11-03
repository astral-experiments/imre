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

As of November 3, 2026, Imre has a paired pass for **8,953 of 13,328 tests (67.2%)** in the original 25 standard library module families.
A **paired pass** means that CPython 3.14.5 passed the original test and Imre passed it at both
`-O0` and `-O3`. The total combines focused and intact suite runs across revisions, including
development branches. Each original test occurrence counts once. Skipped and omitted tests remain
in the denominator. The [omission decisions](docs/stdlib-omissions.md) explain each original included in the omitted column.

| Module | Paired passes | Upstream tests | Omitted | How the tests ran |
| --- | ---: | ---: | ---: | --- |
| `argparse` | 1,887 | 1,893 | 6 | All 1,887 eligible originals have paired passes across public revisions. The translations test ran the real pygettext child and produced all 32 expected messages. Six explicitly documented compiled-entry omissions remain in the 1,893-test denominator. A complete run on one revision is tracked separately. |
| `collections` | 640 | 659 | 9 | [ChainMap lookup and Counter copying](benchmarks/cpython_stdlib/full/collections/receipts/2026-11-02/chainmap-counter/) passed on stock and boxed native O0/O3, including all six Counter pickle subtests. Earlier focused and intact runs contribute to the total. |
| `csv` | 122 | 128 | 0 | Complete suite on `main`, 122 passed and six matched upstream skips. |
| `dataclasses` | 251 | 276 | 0 | The generated constructor kept the expected annotation on both access paths. Changing the annotation made the original assertion fail on all three engines. |
| `datetime` | 1,006 | 1,098 | 0 | The complete 92-parent TestDateTime_Fast class added eleven passes, including timestamp conversions across DST gaps and folds. One malformed packed-year case still errors on both native levels. |
| `fnmatch` | 24 | 24 | 0 | Complete suite on `main`, all 24 passed. |
| `functools` | 91 | 329 | 0 | The intact 325-case primary module passed 89 in all four modes. Two more original cases passed in separate runs. |
| `hashlib` | 77 | 82 | 0 | The cumulative resource-enabled inventory includes 77 distinct pairs. Under the default stock resource policy, intact ordinary O0 and O3 each record 67 passes, one threaded-hashing error and 14 skips. The real builtin MD5 factory is verified at both native levels. |
| `importlib` | 329 | 1,346 | 0 | Seventy-eight more resource tests passed across eight module runs. Sixty-five needed the pinned `zipfile` on the runtime path, and 13 also needed it during compilation. Two originals in `test_files` still errored on Imre. |
| `io` | 530 | 1,038 | 2 | Two intact Python FileIO classes added 48 paired passes. Nine selected C FileIO parents also passed. One stock-matched skip remains outside the paired count. |
| `itertools` | 112 | 138 | 0 | Selected runs account for 112 original passes. Public 90955053 resolved the typing.Iterable import. The unchanged 137-case primary now reaches a module-level type alias in _pyrepl/types.py and stops before discovery at both native levels. A separate GIL companion supplies the 138th registration. |
| `json` | 214 | 216 | 0 | All 34 unmodified CLI originals passed, with 32 child subtests and 64 tool subprocesses per lane. The O0/O3 children ran native code. Two help cases matched the upstream usage-prefix assertion while their full stdout differed from stock. |
| `logging` | 232 | 272 | 0 | Seven more originals passed from the full upstream logging test file in separate stock and native processes. They cover task names, formatter precision, shutdown and recursion. |
| `os` | 406 | 579 | 0 | Two additional FileTests originals passed with explicit input and output splice offsets. Linux transferred the expected six bytes and preserved the file position. |
| `pathlib` | 974 | 1,369 | 1 | Four more `glob` and `rglob` tests passed with recursive symlinks after complete discovery of all 1,369 originals. All three owners exited cleanly, and filesystem observations confirmed the links and teardown in all twelve fixtures. |
| `re` | 147 | 165 | 7 | Seven CPython compiler and bytecode checks have explicit omissions. The 147 paired originals include focused and complete runs. |
| `shutil` | 154 | 223 | 0 | Stock and public native O0/O3 each discovered all 223 originals and ran the same 49 additional tests. All three owners exited 0 with identical parent callbacks and four successful subtests. |
| `socket` | 61 | 746 | 0 | Earlier and focused cases total 61. Four native modes pass 14/19 resolver originals, including the new service database lookup. The 746-case discovery run executed no test bodies. |
| `subprocess` | 289 | 353 | 0 | Five more Linux subprocess originals passed on one public revision. Fifteen traced owners each reaped the setup child and launched no body child, as the original tests require. Ten additional native runs passed without instrumentation. |
| `tempfile` | 108 | 118 | 2 | All 118 originals finished in ordinary and boxed O0/O3. Each native run passed 108 and skipped 10. Stock passed 112 and skipped six. Two extra native skips are documented omissions. The other two require working os.fork and os.spawnl. |
| `textwrap` | 68 | 68 | 0 | Complete suite on `main`, all 68 passed. |
| `threading` | 83 | 228 | 1 | All 228 original registrations ran in ordinary and boxed builds at both levels. Each passed 83, skipped 32 and reported failures on 113 IDs. The bound worker callback still blocks 58 Bunch fixture cases. |
| `typing` | 700 | 712 | 0 | The original generic parameter-detection test and all fourteen of its child checks passed on stock and both ordinary native builds. |
| `unittest` | 275 | 1,090 | 0 | The intact package passed 160 cases. On public #3601, the unchanged test_assertions and test_case files ran all 150 originals with runtime imports. Their 109 paired passes added 81 distinct cases. Another 34 original mock methods passed in focused runs. |
| `urllib` | 173 | 178 | 0 | Both unchanged upstream scripts completed on public `main` at O0 and O3 with ordinary and runtime imports. All 178 tests matched stock in order, including 173 successful parents, three with only child outcomes, and two Windows skips. |
| **Total** | **8,953** | **13,328** | **28** | **4 complete suites match stock on `main`** |

See the [standard library coverage report](docs/stdlib.md) for the Linux test
results and import coverage recorded in this repository.

### Additional module families

CPython 3.14.5 discovers **1,217 test occurrences** in these six families on Linux. They use the same pairing rule and have a separate total. A dash marks a pending native baseline.

| Module | Paired passes | Upstream tests | Coverage |
| --- | ---: | ---: | --- |
| `contextlib` | 148 | 149 | All 58 async originals and 90 of 91 synchronous originals have paired passes. The module entry point passed nine of the ten `TestSuppress` cases on public Imre. The remaining case exercises exception groups. |
| `inspect` | 0 | 362 | Stock ran 362 originals, with 361 passes and one skip. On public revision `00755c8c`, native compilation stopped before discovery, and a compiled import stopped at `_pickle`. Separate replays passed 44 tests in renamed modules. |
| `ssl` | 33 | 195 | Three original context setup tests passed with combined certificate trust sources. Each process registered all 195 default-resource tests. |
| `types` | 120 | 141 | The original union instantiation case passed after all 129 Linux tests registered on stock and both native builds. |
| `warnings` | 177 | 187 | The [intact suite](benchmarks/cpython_stdlib/full/warnings/receipts/76aed745/intact/verify.py) finished all 187 originals. Stock passed 181 and skipped six. Native O0 and O3 each passed 177, shared the six stock skips, and skipped four CPython-only tests. All 18 subtests passed. No warnings omissions. |
| `weakref` | 169 | 183 | All 46 WeakSet originals and 123 weakref originals passed. Seven [CPython-specific tests](docs/stdlib-omissions.md#weakref) have individual omission decisions. Seven other originals remain unpaired. |
| **Measured total (6 of 6 families)** | **647** | **1,217** |  |

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
