# CPython unittest suite

The [manifest](UPSTREAM.json) pins all 38 files in CPython 3.14.5's
[test_unittest package](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_unittest),
including its testmock package, namespace discovery fixtures and support
modules. The [license](upstream/LICENSE) is the release's original license.

## Reference

From the repository root, use a test-enabled interpreter built from the pinned
CPython source tree:

```sh
/path/to/python -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/unittest/UPSTREAM.json --result /tmp/unittest.json
```

The runner verifies both the vendored blobs and the interpreter's test files,
invokes the package's load_tests hook, and compares every discovered ID with
regrtest's JUnit report. Missing or extra occurrences are errors. Repeated IDs retain all of their outcomes.

[Linux reference](reference-linux.json): **1,090 IDs in 24 modules**, 1,088
passed, two skipped. Both skips come from TestBreakSignalIgnored, whose fixture
ignores SIGINT. Each ID retains its original skip reason. These results are all
CPython reference executions; they confer no Imre coverage.

## Native execution

```sh
/path/to/python -B benchmarks/cpython_stdlib/full/unittest/run.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
    --clang /path/to/clang --result /tmp/native-unittest.json
```

Both optimization levels run the two original `TestResult` methods for stopping
and run boundaries. Their complete bodies, result objects and checks run in the
native process. CPython applies the upstream class fixture that disables color
before launching that process.

The unchanged `Test_TestCase.testAssertRaisesNoExceptionType` exercises five
invalid calls. Its nested assertions, validation, and catches run in Imre under
the upstream host fixture runner.

Nine `TestLongMessage` methods run under the original CPython class and setup.
Each tests all four combinations of long/short diagnostics and omitted/explicit
messages. The bound operation executes in a native child, which returns the
actual assertion failure and its message. The original helper checks those
messages on the host. For `testAssertRaises`, the original host callback returns
normally before Imre performs the missing-exception check. These nine are
host-driven native operations.

At each of O0 and O3: 0 fully native passes, 3 native bodies with host
fixtures, 9 host-driven native operations (36 calls), 1,076 reference-only and
2 reference skips. The [versioned evidence reports](native-linux.json) retain
all 2,180 records and their [field definitions](../../NATIVE_EVIDENCE.md).

The [context control](context_protocol.py) compares caught errors, stored managers, argument order and early exits with CPython at both levels. It adds no upstream IDs.

## Exception-family integration controls

`run.py` compares the shipped context protocol against pinned CPython at
both optimization levels. It exercises nested mismatched handlers and the
identity of the original caught object for LookupError, NameError,
UnicodeEncodeError and shutil's OSError subclasses.
On a build that also includes [native CSV](https://github.com/astral-sh/imre/pull/468)
or [structured import errors](https://github.com/astral-sh/imre/pull/590), add
`--exception-family csv` or `--exception-family import`. The selected
control must compile, run, and match the reference; it contributes no upstream IDs.
