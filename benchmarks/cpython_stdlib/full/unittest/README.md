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

Five original `Test_TestCase` methods run their entire bodies and native
assertions: `testAssertMultiLineEqual`, `testAssertEqualSingleLine`,
`testAssertEqualwithEmptyString`, `testAssertEqualMultipleLinesMissingNewlineTerminator`,
and `testAssertEqualMultipleLinesMismatchedNewlinesTerminators`. Each forces
an actual mismatch and checks the diff. The [equality control](equality_protocol.py)
compares complete failure messages, including first differing and extra list
elements, Unicode, truncation, custom messages, and a subclass's failure class.
It adds no upstream IDs.

At each of O0 and O3: 0 fully native passes, 8 native bodies with host
fixtures, 9 host-driven native operations (36 calls), 3 compile blockers,
1,068 reference-only and 2 reference skips. The [versioned evidence reports](native-linux.json) retain
all 2,180 records and their [field definitions](../../NATIVE_EVIDENCE.md).

Three additional complete equality methods are attempted at each level.
`testAssertMultiLineEqualTruncates` constructs a TestCase with a method name;
`testAssertEqual_diffThreshold` and `testAssertEqual_shorten` both reach
`addCleanup`. Each record retains the original passing reference,
the native diagnostic, source line, and method hash.

The [context control](context_protocol.py) compares caught errors, stored managers, argument order and early exits with CPython at both levels. It adds no upstream IDs.

## Exception-family integration controls

`run.py` compares the shipped context protocol against pinned CPython at
both optimization levels. It exercises nested mismatched handlers and the
identity of the original caught object for LookupError, NameError,
UnicodeEncodeError and shutil's OSError subclasses. The default gate also checks
the identity, name and path of [structured import errors](https://github.com/astral-sh/imre/pull/590)
and [csv.Error](https://github.com/astral-sh/imre/pull/468) against unrelated handlers.
These controls must compile, run, and match the reference; they contribute no upstream IDs.

### The regrtest failure fixture

[support_exception.py](support_exception.py) checks the original `test.support`
exception classes against the way regrtest constructs them. It verifies three
CPython blobs, then compares the installed classes with byte-for-byte copies
of their declarations. Both controls pass mutable lists of `(test, traceback)`
pairs. One leaves `stats` as `None`; the other passes regrtest's own `TestStats`.
They check parent and child catches, sibling rejection, argument order, and
whether mutations are visible through the exception's attributes and `args`.

```sh
"$CPYTHON" -B benchmarks/cpython_stdlib/full/unittest/support_exception.py \
    --cpython-source "$CPYTHON_SOURCE" --result /tmp/support-exception.json
```

Add `--imre "$IMRE"` for ordinary and reachable-only compiler diagnostics.
For execution, also supply `--runtime "$IMRE_RUNTIME" --clang "$CLANG"`:
the helper compares stdout, stderr and exit status at O0 and O3 in both
modes and exits unsuccessfully on a blocker or mismatch. These are fixture
controls, so none changes the 1,090-test denominator. A successful isolated
control does not establish that a complete module imports or runs.
