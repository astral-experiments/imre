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

As of November 4, 2026, Imre has a paired pass for **11,440 of 16,202 tests (70.6%)** across the 44 standard library module families tracked below.
The first table covers the original 25 families and the second covers 19 additional families.
A **paired pass** means that CPython 3.14.5 passed the original test and Imre passed it at both
`-O0` and `-O3`. The total combines focused and intact suite runs across revisions, including
development branches. Each original test occurrence counts once. Skipped and omitted tests remain
in the denominator. The [omission decisions](docs/stdlib-omissions.md) explain each original included in the omitted column.

| Module | Paired passes | Upstream tests | Omitted | How the tests ran |
| --- | ---: | ---: | ---: | --- |
| `argparse` | 1,887 | 1,893 | 6 | All 1,887 eligible originals have paired passes across public revisions. The translations test ran the real pygettext child and produced all 32 expected messages. Six explicitly documented compiled-entry omissions remain in the 1,893-test denominator. A complete run on one revision is tracked separately. |
| `collections` | 641 | 659 | 9 | The async-generator ABC original passed on public #4804. All 40 originals in that ABC cohort now have a paired result or an individual omission decision. |
| `csv` | 122 | 128 | 2 | The full suite passed 124/128 on stock and 122/128 at O0 and O3. Four cases skipped on all three engines. Two additional CPython-only cases skipped on Imre. |
| `dataclasses` | 263 | 276 | 0 | Both TestAbstract methods passed on stock and public Imre at O0 and O3. Generated ordering now satisfies the abstract interface. The other method already passed on an earlier public build and was missing from the ledger. |
| `datetime` | 1,043 | 1,098 | 0 | Four unchanged Fast date and datetime constructor parents passed public #4751 at O0 and O3. Stock passed the matching four in its six-parent group. |
| `fnmatch` | 24 | 24 | 0 | Complete suite on `main`, all 24 passed. |
| `functools` | 178 | 329 | 0 | Six original partial representation cases passed against the Python and C implementations, including recursive representation and mutation during representation. Each registered test ran separately on stock and on public Imre at both optimization levels. |
| `hashlib` | 77 | 82 | 0 | The cumulative resource-enabled inventory includes 77 distinct pairs. Under the default stock resource policy, intact ordinary O0 and O3 each record 67 passes, one threaded-hashing error and 14 skips. The real builtin MD5 factory is verified at both native levels. |
| `importlib` | 329 | 1,345 | 0 | Seventy-eight more resource tests passed across eight module runs. Sixty-five needed the pinned `zipfile` on the runtime path, and 13 also needed it during compilation. Two originals in `test_files` still errored on Imre. The Linux roster contains 1,345 original tests. The Windows registry module requires winreg and skips during Linux discovery. |
| `io` | 706 | 1,038 | 2 | The complete 182-test memoryio module passed 177 parents at O0 and O3. Both native builds skipped five. All five additional passes come from the buffer-export changes in PR #4981. |
| `itertools` | 112 | 138 | 0 | Selected runs account for 112 original passes. Public 90955053 resolved the typing.Iterable import. The unchanged 137-case primary now reaches a module-level type alias in _pyrepl/types.py and stops before discovery at both native levels. A separate GIL companion supplies the 138th registration. |
| `json` | 214 | 216 | 0 | All 34 unmodified CLI originals passed, with 32 child subtests and 64 tool subprocesses per lane. The O0/O3 children ran native code. Two help cases matched the upstream usage-prefix assertion while their full stdout differed from stock. |
| `logging` | 232 | 272 | 0 | Seven more originals passed from the full upstream logging test file in separate stock and native processes. They cover task names, formatter precision, shutdown and recursion. |
| `os` | 440 | 579 | 0 | All 12 timerfd parents passed on stock and Imre O0. O3 passed 11 and failed test_timerfd_ns_poll. The fexecve parent also passed on all three, for 12 new paired passes. |
| `pathlib` | 974 | 1,369 | 1 | Four more `glob` and `rglob` tests passed with recursive symlinks after complete discovery of all 1,369 originals. All three owners exited cleanly, and filesystem observations confirmed the links and teardown in all twelve fixtures. |
| `re` | 147 | 165 | 7 | Seven CPython compiler and bytecode checks have explicit omissions. The 147 paired originals include focused and complete runs. |
| `shutil` | 154 | 223 | 0 | Stock and public native O0/O3 each discovered all 223 originals and ran the same 49 additional tests. All three owners exited 0 with identical parent callbacks and four successful subtests. |
| `socket` | 67 | 746 | 0 | All ten descriptor ownership and duplication originals passed on stock and public Imre at O0 and O3. The new ledger entry is `GeneralModuleTests.test_socket_fileno`. |
| `subprocess` | 302 | 353 | 3 | Both original empty-environment cases passed on stock and on public Imre at O0 and O3. They exercise child execution when the supplied environment has no PATH. |
| `tempfile` | 110 | 118 | 2 | All three runs registered the same 118 original parents. CPython passed 112 and skipped six. Public Imre passed 110 and skipped eight at both levels. |
| `textwrap` | 68 | 68 | 0 | Complete suite on `main`, all 68 passed. |
| `threading` | 83 | 228 | 1 | All 228 original registrations ran in ordinary and boxed builds at both levels. Each passed 83, skipped 32 and reported failures on 113 IDs. The bound worker callback still blocks 58 Bunch fixture cases. |
| `typing` | 710 | 712 | 1 | All three ordinary module runs registered 712 parents, including the same 11 doctests with 43 examples. Stock passed 712. Each native run passed 706, errored on four and skipped two. Six named successes are new to the accumulated ledger. |
| `unittest` | 597 | 1,090 | 0 | Two public workers passed four additional mock-helper originals on stock and both native profiles. One covers dataclass hashing. The other three appeared in a complete 84-parent replay and an independent selected run. |
| `urllib` | 173 | 178 | 0 | Both unchanged upstream scripts completed on public `main` at O0 and O3 with ordinary and runtime imports. All 178 tests matched stock in order, including 173 successful parents, three with only child outcomes, and two Windows skips. |
| **Total** | **9,653** | **13,327** | **34** | **3 complete suites match stock on `main`** |

See the [standard library coverage report](docs/stdlib.md) for the Linux test
results and import coverage recorded in this repository.

### Additional module families

CPython 3.14.5 discovers **2,875 test occurrences** across these 19 families on Linux. They use the same pairing rule and have a separate total.

| Module | Paired passes | Upstream tests | Omitted | How the tests ran |
| --- | ---: | ---: | ---: | --- |
| `base64` | 38 | 41 | 0 | Both native runs of the full suite passed 38 of 41 originals. The five new passes cover command-line decoding, file and stdin encoding, help, and invalid flags. Two originals errored on b16encode and one skipped. Stock passed all 41. |
| `configparser` | 326 | 344 | 0 | Stock and both native builds passed the same 19 original multiline parsing tests on public main. All 19 are new to the ledger. |
| `contextlib` | 148 | 149 | 0 | All 58 async originals and 90 of 91 synchronous originals have paired passes. The module entry point passed nine of the ten `TestSuppress` cases on public Imre. The remaining case exercises exception groups. |
| `copy` | 74 | 81 | 0 | The complete public suite reached 74 of 81. A second Ruff worker reproduced `test.test_copy.TestCopy.test_deepcopy_dict_subclass` (occurrence 1) on stock and public Imre at both optimization levels. |
| `fractions` | 50 | 50 | 0 | The ordinary fractions module ran all 50 original parents. Every parent passed on pinned stock and on public Imre O0 and O3. |
| `glob` | 22 | 24 | 0 | On Linux, CPython and both native builds passed 22 of the 24 tests in the complete module, including the open-file stress test. All three skipped the two Windows tests. |
| `gzip` | 29 | 75 | 0 | The complete 75-parent gzip module passed 29 parents on public Imre at O0 and O3. CPython passed all 75. Twenty-six passes are new to the ledger. |
| `inspect` | 185 | 362 | 0 | Two independent public builds passed all seven TestGetcallargsMethods originals. Four new keys cover plain, keyword-only, variable-keyword and mixed argument binding. |
| `math` | 21 | 89 | 0 | Two independent public #5162 replays ran all 89 originals at stock, O0 and O3. The same 21 passed at both native levels, including all 11 isclose originals. One FMA parent emitted subtest errors. |
| `ssl` | 49 | 195 | 0 | Stock and public Imre at O0 and O3 passed `test.test_ssl.ContextTests.test_load_dh_params` (occurrence 1). The source qualification also exercised real TLS handshakes using the loaded parameters. |
| `struct` | 8 | 43 | 0 | The complete suite ran the same 43 original tests on all three engines. Stock CPython passed all 43. Public Imre O0 and O3 passed the same eight. |
| `tokenize` | 53 | 130 | 0 | The unchanged test_max_indent passed on stock and public Imre at O0 and O3. It adds one new paired pass. |
| `tomllib` | 17 | 17 | 0 | All 17 originals passed on stock and public O0/O3 through both the package entry point and the three explicit upstream test modules. |
| `types` | 134 | 141 | 1 | Two Linux workers passed the integer and float locale-formatting originals at stock and both native levels with genuine en_US numeric data. |
| `uuid` | 38 | 120 | 0 | All three engines ran 120 originals. Stock CPython passed 49. Public Imre O0 and O3 passed the same 38, including 11 newly paired UUID generation and CLI cases. All three skipped 71. |
| `warnings` | 181 | 187 | 0 | The intact default suite paired 177/187. Four additional async originals passed with `-X context_aware_warnings=1` on pinned stock and public O0 and O3. Each flag-enabled run registered all 187 originals and completed teardown. |
| `weakref` | 169 | 183 | 7 | All 46 WeakSet originals and 123 weakref originals passed. Seven [CPython-specific tests](docs/stdlib-omissions.md#weakref) have individual omission decisions. Seven other originals remain unpaired. |
| `zipfile` | 204 | 383 | 0 | Three additional originals now pass on the public compiler in both modes. They cover the good password and two forms of CRC failure in deflated ZIP entries. Both stored-entry controls also passed. |
| `zoneinfo` | 41 | 261 | 0 | All 18 originals in WeirdZoneTest and CWeirdZoneTest passed on CPython and public Imre at O0 and O3 after LZMA decompression landed. The cumulative count includes the 23 previously passing properties. |
| **Measured total (19 of 19 families)** | **1,787** | **2,875** | **8** |  |

### Whole-suite baselines for later additions

We track these families separately to keep the original 16,202-test baseline comparable across revisions. Each row comes from complete runs of the upstream module.

| Module | Paired passes | Upstream tests | Stock CPython | Imre O0 and O3 |
| --- | ---: | ---: | --- | --- |
| `pyexpat` | 33 | 60 | 47 passed, 13 skipped | 33 passed, 13 errors, 14 skipped |
| `zlib` | 57 | 76 | 74 passed, 2 skipped | 57 passed, 15 errors, 4 skipped |

Seven XML encoding tests account for the increase from 26 to 33 passes. Both independent Linux workers registered all 60 originals. One class-setup skip occurred before its method ran, so unittest reported 59 tests. [Public Imre revision](https://github.com/astral-experiments/imre/commit/56f7c1d949541a578424c7fdb6db103c7b30beab)

The zlib suite retained 55 earlier passes and added two tests for copying decompressors. Two Linux workers reproduced the 57 passing originals across the earlier suite, the public copy fix, and a complete later run. [Public copy fix](https://github.com/astral-experiments/imre/pull/5158) and [full-suite revision](https://github.com/astral-experiments/imre/commit/dab64ec6ec1617f7cf6cca400351881e13f90044).

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

On Linux, a source build uses `/bin:/usr/bin` to find Clang and its linker
when the calling environment omits `PATH`. To install against tools elsewhere,
set `IMRE_DEFAULT_TOOL_PATH` when building Imre, either in the build environment
or in Cargo's `[env]` configuration. Use colon-separated absolute directories
containing Clang and the linker. An explicit runtime `PATH` takes precedence.
This search path applies only to the compiler subprocess.

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
