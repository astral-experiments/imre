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

As of November 7, 2026, Imre has a paired pass for **12,889 of 16,202 tests (79.6%)** across the 44 standard library module families tracked below.
The paired pass rate is 78.3% (10,429 of 13,327) for the original 25 families and 85.6% (2,460 of 2,875) for the 19 additional families.
A **paired pass** means that CPython 3.14.5 passed the original test and Imre passed it at both
`-O0` and `-O3`. The total combines focused and intact suite runs across revisions, including
development branches. Each original test occurrence counts once. Skipped and omitted tests remain
in the denominator. The [omission decisions](docs/stdlib-omissions.md) cover 79 originals in these 44 families and [eight supplemental Decimal originals](docs/stdlib-omissions.md#decimal).
At either optimization level, a test can exercise typed native code, boxed native code that calls the runtime, or Python source loaded during execution. [Compiler design](docs/design.md#boxed-compilation)

The [supplemental `test_urllib2` decision](docs/stdlib-omissions.md#test_urllib2) covers the single packaging-specific error in a separate 80-case replay.

On the campaign's x86-64 Linux runner, stock CPython 3.14.5 registered **16,191** of these originals.
It passed **15,161**, skipped 1,023, and reported 7 errors.
Across the 44 families, **12,798 of the 15,161 stock Linux passes (84.4%)** have paired passes, leaving **847** more to reach 90%. The other 91 paired originals skipped or recorded an error in that stock run.

| Module | Paired passes | Upstream tests | Omitted | How the tests ran |
| --- | ---: | ---: | ---: | --- |
| `argparse` | 1,887 | 1,893 | 6 | All 1,887 eligible originals have paired passes across public revisions. The translations test ran the real pygettext child and produced all 32 expected messages. Six explicitly documented compiled-entry omissions remain in the 1,893-test denominator. A complete run on one revision is tracked separately. |
| `collections` | 641 | 659 | 9 | The async-generator ABC original passed on public #4804. All 40 originals in that ABC cohort now have a paired result or an individual omission decision. |
| `csv` | 122 | 128 | 2 | The full suite passed 124/128 on stock and 122/128 at O0 and O3. Four cases skipped on all three engines. Two additional CPython-only cases skipped on Imre. |
| `dataclasses` | 273 | 276 | 0 | The whole-suite run for merged #5754 passed 272 of 276 at both native levels, with one failure, one error and two skips. Thirteen generated-docstring tests moved from error to pass. All thirteen were already counted in the cross-revision ledger. A complete replay on `7a67ce5b` also paired 272. The earlier `TestCase.test_not_other_dataclass` pass failed its assertion about the unpacking TypeError because Imre reported that the Point3D object was not iterable. [Merged docstring change](https://github.com/astral-experiments/imre/pull/5754). |
| `datetime` | 1,043 | 1,098 | 21 | Four unchanged Fast date and datetime constructor parents passed public #4751 at O0 and O3. Stock passed the matching four in its six-parent group. The [21 omissions](docs/stdlib-omissions.md#datetime) cover CPython C API requirements. [Two extension-module tests](docs/stdlib-omissions.md#datetime-extension-module-tests) remain in scope. |
| `fnmatch` | 24 | 24 | 0 | Complete suite on `main`, all 24 passed. |
| `functools` | 316 | 329 | 2 | With the native `_functools` provider, the earlier 325-method run recorded 315 passes, seven errors and three skips. The shared reduce implementation added the original Python reduce-initializer test when the initializer was supplied by keyword. [#5850](https://github.com/astral-experiments/imre/pull/5850) · [#5890](https://github.com/astral-experiments/imre/pull/5890) |
| `hashlib` | 78 | 82 | 0 | Under default resources, 67 of 69 stock passes paired on Imre. Of those, 66 were already among the 77 resource-enabled pairs. The [earlier public replay](benchmarks/cpython_stdlib/full/hashlib/README.md#ordinary-stock-replay-at-7f886453) supplies the additional pair, `HashLibTestCase.test_hash_disallow_instantiation`, giving 78 distinct pairs across policies. Both default native runs skipped 14 tests and errored in `test_threaded_hashing`. |
| `importlib` | 329 | 1,345 | 6 | Seventy-eight more resource tests passed across eight module runs. Sixty-five needed the pinned `zipfile` on the runtime path, and 13 also needed it during compilation. The [six omissions](docs/stdlib-omissions.md#importlib) cover two bytecode-only resource fixtures and four CPython multi-phase extension tests. The Linux roster contains 1,345 original tests. The Windows registry module requires winreg and skips during Linux discovery. |
| `io` | 949 | 1,038 | 4 | The intact stock, O0 and O3 runs on #5867 reproduced all 948 earlier pairs in order. A 12-test selection on #5920 added `CMiscIOTest.test_io_after_close`. [#5867](https://github.com/astral-experiments/imre/pull/5867) · [#5920](https://github.com/astral-experiments/imre/pull/5920) |
| `itertools` | 113 | 138 | 0 | Three independent processes passed `TestBasicOps.test_tee_dealloc_segfault` (occurrence 1) on stock and on public #5838 at O0 and O3, adding one to the 112 earlier pairs. At this checkpoint the whole-suite replay, which includes the 20-million-item `test_tee_del_backward`, was still running. [Replay revision](https://github.com/astral-experiments/imre/commit/fd065c64ea74afac6ed5f95e7619cf83761660e5). |
| `json` | 215 | 216 | 0 | Stock and public Imre at O0 and O3 each ran all 216 originals in separate working directories, passed 215 and skipped the same address-space test. The one new pair was `TestPyRecursion.test_highly_nested_objects_encoding`. The separate encoder candidate added no pairs. Earlier runs of all 34 CLI originals recorded 32 child subtests and 64 tool subprocesses per lane. The O0/O3 children ran native code. Two help cases matched the upstream usage-prefix assertion while their full stdout differed from stock. [Tested public revision](https://github.com/astral-experiments/imre/commit/af2d9fc74ef99c453a76948aeb42927712c47b87). |
| `logging` | 232 | 272 | 0 | Seven more originals passed from the full upstream logging test file in separate stock and native processes. They cover task names, formatter precision, shutdown and recursion. |
| `os` | 444 | 579 | 0 | All 12 timerfd parents passed on stock and Imre O0. O3 passed 11 and failed test_timerfd_ns_poll. The fexecve parent also passed on all three, for 12 new paired passes. The POSIX `PosixTester.test_getgroups` original added one more. All three `CPUCountTests` already passed at O0 and O3, including the check that removing one CPU from the affinity mask lowers `os.process_cpu_count()` by one. [CPU count revision](https://github.com/astral-experiments/imre/commit/077899d9864948c2a72ef1b3b55f8d89f2800eda). |
| `pathlib` | 974 | 1,369 | 1 | All 1,369 originals ran in order on Linux. Stock passed 978 and skipped 391. O0 and O3 each reproduced all 974 earlier pairs, skipped 392 and errored on three `test_parts_multithreaded` cases at `Thread.start()`, before any worker read `path.parts`. The extra skip was the CPython-only lazy-import test. [Intact replay](https://github.com/astral-experiments/imre/commit/3e1a1db51d3ebea1cab66a1afb90eca9c6596d91) |
| `re` | 147 | 165 | 7 | Seven CPython compiler and bytecode checks have explicit omissions. The 147 paired originals include focused and complete runs. |
| `shutil` | 174 | 223 | 0 | All 173 originals that passed on the fixed Linux stock build now pass on Imre at O0 and O3. The new Zstandard test created four archives and extracted them eight times on each engine. A separate stock build verified the bzip2 archive test, bringing the raw total to 174. [#5899](https://github.com/astral-experiments/imre/pull/5899) |
| `socket` | 99 | 746 | 0 | The full Linux replay paired 96 originals. `GeneralModuleTests.test_socket_methods` gained a pass through the native class descriptors. Three earlier hostname and IDNA passes skipped on both stock and Imre in this fixture. Tests requiring a shared Python worker thread still stop before their bodies. [Resolver coverage](https://github.com/astral-experiments/imre/pull/5897) · [Socket descriptors](https://github.com/astral-experiments/imre/pull/5949) |
| `subprocess` | 302 | 353 | 3 | Both original empty-environment cases passed on stock and on public Imre at O0 and O3. They exercise child execution when the supplied environment has no PATH. |
| `tempfile` | 110 | 118 | 2 | All three runs registered the same 118 original parents. CPython passed 112 and skipped six. Public Imre passed 110 and skipped eight at both levels. |
| `textwrap` | 68 | 68 | 0 | Complete suite on `main`, all 68 passed. |
| `threading` | 97 | 228 | 1 | An earlier whole-suite run qualified 91 passes. Four subsequent shutdown and atexit cases matched their stock child output. Restoring the module’s public names added `test__all__`. The main-thread shutdown test was already passing on public. Each isolated child printed the two expected lines and exited successfully. These two results bring the cumulative ledger to 97. Two earlier passes, `test_repr_stopped` and `test_leak_without_join`, passed when prelinked and errored when loaded from disk on the same public revision. The one omission accepted an unrelated child compilation failure. [Public module and shutdown runs](https://github.com/astral-experiments/imre/commit/20ef45d4fd1f160df1de920bbb7da031ce2f558a) · [Earlier shutdown replay](https://github.com/astral-experiments/imre/commit/c047d423c21207a400beb165da196bd8b451117c). |
| `typing` | 710 | 712 | 2 | All three ordinary module runs registered 712 parents, including the same 11 doctests with 43 examples. Stock passed 712. Each native run passed 706, errored on four and skipped two. Six named successes are new to the accumulated ledger. Upstream-defined CPython-only guards skipped two tests on Imre. Both passed on stock and have [individual omission decisions](docs/stdlib-omissions.md#typing). |
| `unittest` | 909 | 1,090 | 0 | The whole installed CPython 3.14.5 test package paired 897 originals on `5ea87479` and 909 on the async-patch source `bbe41b18`, including 408 of 558 `unittest.mock` cases. The first 17 additions to the previous 880 were already passing ThreadingMock cases. The async-patch change added 12 cases for coroutine targets and specs. On public #5924 all 29 passed together at O0 and O3. Both whole runs on each revision executed all 946 registered originals and two loader skips. The missing `_testmultiphase` extension prevented discovery of 144 originals. [Prior ledger](https://github.com/astral-experiments/imre/pull/5790) · [ThreadingMock](https://github.com/astral-experiments/imre/pull/5860) · [Async patch selection](https://github.com/astral-experiments/imre/pull/5924). |
| `urllib` | 173 | 178 | 0 | Both unchanged upstream scripts completed on public `main` at O0 and O3 with ordinary and runtime imports. All 178 tests matched stock in order, including 173 successful parents, three with only child outcomes, and two Windows skips. |
| **Total** | **10,429** | **13,327** | **66** | **4 complete suites match stock on `main`** |

See the [standard library coverage report](docs/stdlib.md) for the Linux test
results and import coverage recorded in this repository.

### Additional module families

CPython 3.14.5 discovers **2,875 test occurrences** across these 19 families on Linux. They use the same pairing rule and have a separate total.

| Module | Paired passes | Upstream tests | Omitted | How the tests ran |
| --- | ---: | ---: | ---: | --- |
| `base64` | 40 | 41 | 0 | Stock passed all 41 originals. Both native runs passed 40, retaining the 38 earlier passes and adding `BaseXYTestCase.test_RFC4648_test_cases` and `BaseXYTestCase.test_b16encode` (occurrence 1 each). The earlier 38 include five CLI tests for decoding, encoding from files and stdin, help, and invalid flags. Only the CPython-specific `LazyImportTest.test_lazy_import` skipped on Imre. A separate Base16 application produced identical output on stock, O0 and O3. [Public replay revision](https://github.com/astral-experiments/imre/commit/259f2ddc8e20cae1e61a0438a77be7d47b4037b1). |
| `configparser` | 327 | 344 | 0 | Of 13 remaining stock-positive originals, `test_inconsistent_converters_state` passed at O0 and O3 on public main. A separate replay confirmed the native compiler flags, executable hashes and normal return. [Tested revision](https://github.com/astral-experiments/imre/commit/fa22aa2b07e03fdd65a1748ee106f9945ae155d4). |
| `contextlib` | 149 | 149 | 0 | All 149 synchronous and asynchronous originals passed together on public Imre, including `test.test_contextlib.TestSuppress.test_exception_groups` (occurrence 1). |
| `copy` | 81 | 81 | 0 | Complete suite. Two independent machines ran all 81 upstream originals on stock and public `b9dfd225` at O0 and O3. All six runs passed and exited successfully. |
| `fractions` | 50 | 50 | 0 | The ordinary fractions module ran all 50 original parents. Every parent passed on pinned stock and on public Imre O0 and O3. |
| `glob` | 22 | 24 | 0 | On Linux, CPython and both native builds passed 22 of the 24 tests in the complete module, including the open-file stress test. All three skipped the two Windows tests. |
| `gzip` | 74 | 75 | 1 | Stock passed all 75 originals. Both native builds retained the prior 73 passes and now pass `test_decompress_limited`, which checks read-ahead after a one-byte read. The remaining original calls CPython’s private prepend helper. [Gzip fix](https://github.com/astral-experiments/imre/pull/5627) · [Omission decision](docs/stdlib-omissions.md#gzip). |
| `inspect` | 282 | 362 | 0 | The public replay verified source retrieval for `collections.abc` in the Imre parent, adding one to the 281 earlier pairs. The `TestMain.test_builtins` subprocess ran host Python and receives no native credit. [#5903](https://github.com/astral-experiments/imre/pull/5903) |
| `math` | 73 | 89 | 0 | The earlier 71 pairs include 70 replayed originals and one historical `test_sumprod_stress` pass. The focused selection added `testLog1p` and `testPerm` at O0 and O3 without rerunning the optional stress test. `testLog1p` already passed on the first parent of #5932, where `testPerm` still errored. [#5719](https://github.com/astral-experiments/imre/pull/5719) · [#5902](https://github.com/astral-experiments/imre/pull/5902) · [#5932](https://github.com/astral-experiments/imre/pull/5932) |
| `ssl` | 54 | 195 | 0 | The complete 195-test replay retained all 53 pairs, including the TLS 1.3 post-handshake-authentication setter, and restored `ContextTests.test_options`. Both native runs recorded 54 passes, 26 skips and 115 errors. [#5872](https://github.com/astral-experiments/imre/pull/5872) · [#5908](https://github.com/astral-experiments/imre/pull/5908) |
| `struct` | 34 | 43 | 0 | Stock passed all 43 tests. Both native builds passed 34, skipped six, failed one and errored on two. The full run retained all 33 earlier passes and confirmed `StructTest.test_repr`. [Earlier 33](https://github.com/astral-experiments/imre/commit/dbeabb7cf3753ccd9de8bf53e7b5d78382d75542) · [Public revision](https://github.com/astral-experiments/imre/commit/29e1ce351ccc3cfc398c24db8d5f0fe4d5ff43f2). |
| `tokenize` | 129 | 130 | 0 | The original `test_random_files` passed on stock and at both public native levels. Each run opened the same ten shipped source files and completed all ten file subtests, reading 501,523 bytes. The remaining original invokes the CLI on Latin-1 source, which the compiler rejected before parsing. [Public run](https://github.com/astral-experiments/imre/commit/9faed4f24231cca67a147ca070e7fd79a71bba51). |
| `tomllib` | 17 | 17 | 0 | All 17 originals passed on stock and public O0/O3 through both the package entry point and the three explicit upstream test modules. |
| `types` | 134 | 141 | 1 | Two Linux workers passed the integer and float locale-formatting originals at stock and both native levels with genuine en_US numeric data. |
| `uuid` | 48 | 120 | 1 | Both native builds passed 47 of 120 originals on the public squash, including 47 of 48 frozen CPython-positive cases. An earlier host also passed the IP-dependent lookup. The private enum layout check has a [documented omission decision](docs/stdlib-omissions.md#uuid). [Public change](https://github.com/astral-experiments/imre/pull/5313). |
| `warnings` | 181 | 187 | 0 | The intact default suite paired 177/187. Four additional async originals passed with `-X context_aware_warnings=1` on pinned stock and public O0 and O3. Each flag-enabled run registered all 187 originals and completed teardown. |
| `weakref` | 169 | 183 | 7 | All 46 WeakSet originals and 123 weakref originals passed. Seven [CPython-specific tests](docs/stdlib-omissions.md#weakref) have individual omission decisions. Seven other originals remain unpaired. |
| `zipfile` | 368 | 383 | 1 | The earlier 367 pairs include 32 Zstandard originals and two archive-execution tests whose Bash fixture launched CPython with `-E` and no `-O` (Python optimization level 0). `TestPath.test_encoding_warnings` added one pair with `-X warn_default_encoding` on stock, O0 and O3. Default runs skipped it. [#5854](https://github.com/astral-experiments/imre/pull/5854) · [#5899](https://github.com/astral-experiments/imre/pull/5899) · [Flagged run revision](https://github.com/astral-experiments/imre/commit/ea7b0bf455931d2d77d41cece413a80050bc66a4) |
| `zoneinfo` | 228 | 261 | 2 | All 261 original registrations completed on public Imre. Both native builds passed 228, including `test.test_zoneinfo.test_zoneinfo.MiscTests.test_pydatetime` (occurrence 1), which exercises the pure-Python datetime provider. Two private-cache tests have [documented omission decisions](docs/stdlib-omissions.md#zoneinfo). [Public change](https://github.com/astral-experiments/imre/pull/5295). |
| **Measured total (19 of 19 families)** | **2,460** | **2,875** | **13** |  |

### Whole-suite baselines for later additions

We track these families separately to keep the original 16,202-test baseline comparable across revisions. Each row comes from complete runs of the upstream module. Across these 33 suites, 2,176 of 3,378 originals have paired passes.

| Module | Paired passes | Upstream tests | Stock CPython | Imre O0 and O3 |
| --- | ---: | ---: | --- | --- |
| `_thread` | 12 | 35 | 35 passed | Both intact runs reached all 35 outcomes and exited 1. Two unstarted-handle checks now raise the expected RuntimeError. All 210 individual selections matched their intact outcomes. [#5873](https://github.com/astral-experiments/imre/pull/5873) |
| `abc` | 61 | 72 | 72 passed | With CPython’s `_py_abc.py` on the import path, both native levels improved from 59 to 61 passes. The two gains were the C and Python occurrences of `TestLegacyAPI.test_abstractproperty_basics`. Three originals failed and eight errored at each level (exit 1). The runner counted four additional subtest failures. [Public fix](https://github.com/astral-experiments/imre/commit/09e41b4a61c52eb8d8f6339b94bc179c8a8b5462). |
| `asyncio.test_subprocess` | 45 | 81 | 81 passed on CPython 3.14.5 built against the same Linux headers with pidfd support | The independent replay of `8a7104c5` retained all 8 pairs from its first parent `95c1068c` and added 37. [#5789](https://github.com/astral-experiments/imre/pull/5789). |
| `binascii` | 32 | 93 | 76 passed, 17 skipped | 32 passed, 8 failures, 36 errors, 17 skipped |
| `bisect` | 23 | 46 | 46 passed | 23 passed, 23 errors. All 23 pure-Python tests passed, including `TestBisectPython.test_lookups_with_key_function`. The 23 errors came from tests that require the missing `_bisect` accelerator. [Casefold fix](https://github.com/astral-experiments/imre/pull/5618). |
| `bz2` | 99 | 101 | 100 passed, 1 reference-count check skipped | On `454b938c`, both native levels registered all 101 originals, passed 99, skipped the reference-count check and errored in one threading test (exit 1). The first parent `06c7cf6b` registered zero originals. [#5800](https://github.com/astral-experiments/imre/pull/5800). |
| `calendar` | 77 | 79 | 77 passed, 2 skipped | 77 passed, 2 skipped. Stock and both native levels passed all 15 command-line tests and skipped the same two tests because the `en_US` locale was unavailable. [Tested revision](https://github.com/astral-experiments/imre/commit/4239d6db737b792352157b9a71ec526db5f12ec5). |
| `cmd` | 4 | 7 | 5 passed, 2 unrun | 4 passed and one skipped. A shared readline class fixture prevented two tests from starting. The two passing doctests ran all 35 examples. [Public revision](https://github.com/astral-experiments/imre/commit/cda138b4a157dd9d6fefaa21f3e5f6353d13db14). |
| `colorsys` | 7 | 7 | 7 passed | 7 passed. [Public revision](https://github.com/astral-experiments/imre/commit/46bd6bc7dd58da8cfb9f17c56c923cdb549ce265). |
| `decimal` | 375 | 732 | 724 passed, 8 skipped | Both native levels completed all 577 registered occurrences on #5751. Each recorded 375 passes, two failures, one error and 199 skips. The other 155 stock occurrences were unregistered. `PyUsabilityTest.test_hash_method` was the sole new pair. [Public merge #5751](https://github.com/astral-experiments/imre/pull/5751). |
| `descr` | 92 | 162 | 157 passed, 3 skipped, 2 expected failures | Both intact runs completed all 162 and exited 1. The first parent segfaulted at `ClassPropertiesAndMethods.test_repr_as_str` after 58 passes. That method now passes. The other 33 passes had already succeeded in a separate run of the 72 later originals. [#5889](https://github.com/astral-experiments/imre/pull/5889). |
| `difflib` | 58 | 58 | 58 passed | 58 passed. Both native builds passed all 38 unit tests and all 20 doctests, including 75 examples. [Public revision](https://github.com/astral-experiments/imre/commit/1ad0cc479d6ab637c7879dae69d12a45c9245b98). |
| `exception_group` | 52 | 52 | 52 passed | All 52 originals passed by filename and through unittest module execution at O0 and O3. The last case checks the class name in the error for `Exception[OSError]`. [Generic aliases](https://github.com/astral-experiments/imre/pull/5933) · [Class subscription](https://github.com/astral-experiments/imre/pull/5948) |
| `float` | 46 | 54 | 52 passed, 2 skipped | On #5804, only `GeneralFloatCases.test_float_ceil` changed, bringing both native levels from 45 to 46 passes. On #5566, stock passed 53 and skipped one. Each native run passed 45, with five errors, one failure and three skips. That run added `FormatTestCase.test_format` to the 44 earlier pairs. [#5566](https://github.com/astral-experiments/imre/commit/5677b8b34aaccde6bc52e020bd1484990c328344) · [#5804](https://github.com/astral-experiments/imre/pull/5804). |
| `getpass` | 24 | 24 | 24 passed | 24 passed. The prompt stream flushes before reading and again after input. On Unix, `getpass` falls back to standard input when it cannot open the controlling terminal. [#5510](https://github.com/astral-experiments/imre/pull/5510). |
| `graphlib` | 17 | 17 | 17 passed | 17 passed. [Public change](https://github.com/astral-experiments/imre/pull/5469). |
| `html` | 2 | 2 | 2 passed | 2 passed. The complete `test_html` module covers `escape` and `unescape`. The parser has its own upstream suite. [Public revision](https://github.com/astral-experiments/imre/commit/2df8197514bebd160e5f80ddcc20a0c438bae9df). |
| `http.client` | 131 | 144 | 139 passed, 5 skipped | 131 passed, 9 errors, 4 skipped |
| `ipaddress` | 211 | 211 | 211 passed | All 211 originals passed at both native optimization levels. [Public merge #5560](https://github.com/astral-experiments/imre/commit/56314e43d8fe671b05f2b0be1a22139fa45a3755). |
| `lzma` | 118 | 121 | 120 passed, 1 skipped | 118 passed, 2 errors, 1 skipped. Both native runs retained all 117 prior passes and added `FileTestCase.test_issue44439`. [Public change](https://github.com/astral-experiments/imre/pull/5489). |
| `operator` | 76 | 110 | 110 passed | Both full public runs retained all 70 earlier pairs. The Python and C variants of the `attrgetter`, `itemgetter` and `methodcaller` signature tests added six. [#5903](https://github.com/astral-experiments/imre/pull/5903) |
| `optparse` | 151 | 154 | 154 passed | 151 passed, 2 skipped, 1 failure. The upstream `Tools/i18n/pygettext.py` found no messages at the module’s virtual source path. [Public revision](https://github.com/astral-experiments/imre/commit/276f0cbe28c37627fa4d0859dfe439303a208d74). |
| `pyexpat` | 34 | 60 | 60 passed | 34 passed, 12 errors, 14 skipped (one during class setup) |
| `queue` | 33 | 162 | 156 passed, 6 skipped | 33 passed, 85 skipped and 44 raised errors. Of the skips, 82 require the C implementation of _queue and three require a large-memory run. [Public revision](https://github.com/astral-experiments/imre/commit/cda138b4a157dd9d6fefaa21f3e5f6353d13db14). |
| `quopri` | 11 | 11 | 11 passed | 11 passed. [Public revision](https://github.com/astral-experiments/imre/commit/8c4745e18f48eda34e0b2662493a630404ada941). |
| `range` | 29 | 29 | 29 passed | The public merge passed all 29 originals and 184 subtests in intact runs at both levels. Its first parent passed 28. The final method checks invalid argument counts. [#5887](https://github.com/astral-experiments/imre/pull/5887) · [#5900](https://github.com/astral-experiments/imre/pull/5900) |
| `runpy` | 22 | 40 | 40 passed | All 40 originals ran on the parent and the merge at O0 and O3. Ten `RunPathTestCase` tests brought the paired count from 12 to 22. Each merge run had one failure and 17 errors. [Public merge #5758](https://github.com/astral-experiments/imre/commit/2a5b0e880d1067ae7f811e537fff17dcf5a07fa8). |
| `selectors` | 70 | 121 | 78 passed, 43 skipped | 70 passed. Both native builds retained all 49 earlier paired passes and passed the 21 explicit `EpollSelectorTestCase` originals. [Public revision](https://github.com/astral-experiments/imre/commit/4c5cb58bf0bbcb1704a484e46b4ffd0be97ff02a) |
| `shlex` | 18 | 19 | 19 passed | 18 passed, 1 skipped |
| `signal` | 16 | 57 | 44 passed, 13 skipped | `SiginterruptTest.test_siginterrupt_off` already passed on the parent. The `signal.pause()` fix on #5925 changed `PosixTests.test_interprocess_signal` from failure to pass. Each native level recorded 16 passes, 33 skips, one failure and seven errors. The O0/O3 labels identify the test runners. The child executables and their effective optimization levels were not recorded. [#5925](https://github.com/astral-experiments/imre/pull/5925) |
| `statistics` | 132 | 400 | 400 passed | All 114 earlier passes held. The 18 additions cover `median`, including its doctest. Each native run recorded 226 originals with errors, one failure and 21 skips. Twenty stock doctests did not register. [Public revision](https://github.com/astral-experiments/imre/commit/9fc3782e546c4217286e0c021ce396f5fe1635f1). |
| `timeit` | 41 | 41 | 41 passed | 41 passed. [Public change](https://github.com/astral-experiments/imre/pull/5474). |
| `zlib` | 57 | 76 | 74 passed, 2 skipped | 57 passed, 15 errors, 4 skipped |

In a separate control for `asyncio.test_subprocess`, the supplied stock CPython build passed 42 tests and skipped the 39 pidfd cases. Both `95c1068c` and `8a7104c5` paired 8 of those 42 passes at O0 and O3. The table counts the 81-test replay with pidfd support once.

The LZMA row reports the complete suite on #5489. In a later focused run, `CompressorDecompressorTestCase.test_pickle` passed on stock and on public Imre at O0 and O3. [Focused replay on #5501](https://github.com/astral-experiments/imre/pull/5501).

Seven XML encoding tests raised the earlier `pyexpat` count from 26 to 33 passes. Both independent Linux workers registered all 60 originals. One class-setup skip occurred before its method ran, so unittest reported 59 tests. [Public Imre revision](https://github.com/astral-experiments/imre/commit/56f7c1d949541a578424c7fdb6db103c7b30beab). The table now includes the earlier namespace-prefix pass, `SetAttributeTest.test_namespace_prefixes`. The complete replay confirmed that all 34 already passed before the DOCTYPE change. [#5198](https://github.com/astral-experiments/imre/pull/5198) · [#5571](https://github.com/astral-experiments/imre/pull/5571).

The zlib suite retained 55 earlier passes and added two tests for copying decompressors. Two Linux workers reproduced the 57 passing originals across the earlier suite, the public copy fix, and a complete later run. [Public copy fix](https://github.com/astral-experiments/imre/pull/5158) and [full-suite revision](https://github.com/astral-experiments/imre/commit/dab64ec6ec1617f7cf6cca400351881e13f90044).

Both native levels retained the earlier 130 passes and passed the TLS 1.3 post-handshake authentication case. [Full-suite revision](https://github.com/astral-experiments/imre/commit/87716da5bdbcc7d0d33f0eb4470ae828e10eb1e8).

Four CRC32 originals raised the whole-suite count from 28 to 32. Stock and both native levels also accepted an intact ZIP member and rejected a corrupted copy. [CRC32 revision](https://github.com/astral-experiments/imre/commit/2674c357fa343457574ce1e9ad701361e65fe98b).

Both native levels skipped the upstream CPython-specific lazy-import test. It stays in the denominator. [Full-suite revision](https://github.com/astral-experiments/imre/commit/87f33807d3a0108053fcd07421856c6aadef4f4f).

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
