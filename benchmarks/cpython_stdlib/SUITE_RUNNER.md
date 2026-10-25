# Running pinned CPython methods in Imre

Import `suite_inventory` and `suite_native` from this directory. Use the release's
test-enabled Python for discovery, shared fixtures and host assertions. Build each
native entry at both `Toolchain(..., level=0)` and `level=3`.

## Run an existing suite in both compiler modes

[suite_modes.py](suite_modes.py) invokes the selected checkout's own runner
twice. It passes `--reachable-only` to native builds on the second invocation.
The owner supplies a runner that accepts `--imre`, `--runtime`, `--clang`,
`--cpython-source` and `--result` and emits a full O0/O3 evidence pair. The
launcher uses that checkout's validator and reference; it copies no outcomes
between modes. For example, from a clean checkout with the pinned Python:

```sh
python3 benchmarks/cpython_stdlib/suite_modes.py \
  --source "$(git rev-parse HEAD)" --entry benchmarks/cpython_stdlib/full/unittest/run.py \
  --reference benchmarks/cpython_stdlib/full/unittest/reference-linux.json \
  --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 \
  --cpython-source /path/to/cpython-3.14.5 --output /tmp/unittest-evidence
```

Choose a fresh output directory. `ordinary.json` and `reachable.json` each
retain every original occurrence and its body, assertions, operation, fixture,
and reason. `source.json` identifies the suite revision, inputs and binary
hashes, and both runner and validator exit codes. A runner failure, missing
report or invalid pair fails the command, even if the other mode succeeds.

The **CI** workflow's optional `unittest_source` dispatch builds an exact Imre
commit on Linux and uploads those three files as `unittest-evidence-linux`.
It uses the suite and validator at the selected commit, including when that
commit predates this launcher. The build step in the dispatch establishes the
compiler's provenance; a local invocation records the supplied binary hashes.

### Verified upstream example

The [Linux example](full/unittest/runner-modes-example-linux.tar.gz) contains
both full reports and the receipt for Imre `ec4622afddbdf0b4b892220eae6580245a2f3291`.
The unchanged CPython method
`test.test_unittest.testmock.testmock.MockTest.test_assert_called_once_message_not_called`
exercises a failing native `Mock.assert_called_once()` and checks its diagnostic
with native assertions. Its original method and default fixture ran under Imre
in all four configurations. To see its record and the actual build commands:

```sh
example=benchmarks/cpython_stdlib/full/unittest/runner-modes-example-linux.tar.gz
tar -xOzf "$example" reachable.json | jq '.[] | .records[] | select(.id | endswith(".MockTest.test_assert_called_once_message_not_called"))'
tar -xOzf "$example" source.json | jq '.modes'
```

Each configuration retained all **1,090** original occurrences: 8 fully native,
8 native bodies with host fixtures, 9 host-driven native operations, 9 blocked,
1,054 reference-only, and 2 original skips. Each mode observed 37 native builds
at O0 and 37 at O3. Both reports matched independent runs byte for byte.
`compiler_commands` counts actual invocations; `runner_mode_flag` records
whether the owner's runner received its own `--reachable-only` switch.
A record whose declared mode disagrees with the compiler, or a missing build
at either optimization level, appears in `mode_errors` and fails the run.

## Discover methods on a known class

To discover method names in native code, pass a statically known class to
`unittest.defaultTestLoader.getTestCaseNames`. The [verified loader example](full/unittest/NATIVE_LOADER.md)
uses CPython's original `bar.test_bar.PassingTest` and documents which module
and suite runner calls still fail.

## A verified upstream example

This is the `test_stop` case used by [the unittest gate](full/unittest/run.py).
The complete original method creates a `unittest.TestResult`, calls `stop()`,
and asserts that `shouldStop` is true. The product object, method body and
boolean equality assertion execute in Imre. The original CPython class setup
disables terminal color for the process it launches. This is a native body with
host-assisted class setup. No implementation of `stop()` runs on the host.

```python
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from full.unittest.run import reference_skips
from suite_evidence import validate_pair
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_evidence, testcase_fixture

# Run from benchmarks/cpython_stdlib with the pinned, test-enabled interpreter.
root = Path("full/unittest").resolve()
source = root / "upstream/Lib/test/test_unittest/test_result.py"
test_id = "test.test_unittest.test_result.Test_TestResult.test_stop"
manifest = json.loads((root / "UPSTREAM.json").read_text())
reference = json.loads((root / "reference-linux.json").read_text())
verify_files(root / "upstream", manifest["files"])
verify_interpreter_tests(manifest)
reports = []
for level in (0, 3):
    tools = Toolchain(Path("/path/to/imre"), Path("/path/to/libimre_runtime.a"),
                      Path("/path/to/clang"), level)
    with TemporaryDirectory() as work:
        execute = native_method(tools, Path(work), source, "Test_TestResult", test_id,
                                testcase_fixture(), operation=("imre", "unittest.TestResult.stop"))
        report = run_evidence(load_suite("test.test_unittest"), {test_id: execute}, reference,
                              level, exceptions=reference_skips(level))
    row, = [r for r in report["records"] if r["id"] == test_id]
    assert row["outcome"] == "pass" and row["classification"] == "native-body-host-fixture"
    assert row["roles"] == {"body": "imre", "assertions": "imre", "operation": "imre", "fixture": "mixed"}
    reports.append(report)
validate_pair(reports, reference)
```

Use `verify_files` on your vendored manifest and `verify_interpreter_tests` on
the interpreter's matching test files before executing a suite. `load_suite`
honors upstream `load_tests` hooks. `cases` walks all instances, including
legitimate repeated IDs.

`native_method` copies the pinned definition as bytes and records its SHA-256.
The fixture is an Imre source prefix ending inside a class named `NativeCase`.
The default fixture inherits the native `TestCase`, including its boolean and
integer equality checks. Module owners may provide their actual typed instance fixture.
A decorated method requires a dedicated adapter; class and module fixtures
execute on CPython. If the test requires populated instance state, reproduce or
transfer that state into the native fixture before crediting its body.

For host-driven operations, pass a callback to `run_evidence` instead.
It receives the original case and bound method after setup. Bind only the
operations implemented by Imre, run the original method, and return evidence
of the actual native calls. See the [versioned evidence contract](NATIVE_EVIDENCE.md)
for per-occurrence records, exact exception policies and rollup validation.
The original `run_in_fixtures` callback interface remains available.

The default fixture supports `with self.assertRaises(ValueError) as cm:` and subsequent reads of `cm.exception`. The unchanged `Test_TestCase.testAssertRaisesNoExceptionType` exercises the default fixture at both levels.

### String and list assertions

The bundled `unittest.TestCase` accepts `self.assertEqual(a, b, msg=None)`
for a pair of strings or two lists whose elements are strings, bytes, or
integers. `assertMultiLineEqual` and `assertListEqual` use the same respective
comparisons. Mismatches raise the case's `failureException` and respect
`longMessage` and `maxDiff`. See the [negative controls](full/unittest/equality_protocol.py)
and [pinned methods and outcomes](full/unittest/README.md).

For the original `string` tests, add their import to the standard fixture.
After verifying the module's manifest as above, pass the callback to
`run_evidence` under its original ID:

```python
case_id = "test.test_string.test_string.ModuleTest.test_capwords"
execute = native_method(
    tools, directory, string_suite / "upstream/Lib/test/test_string/test_string.py",
    "ModuleTest", case_id, "import string\n" + testcase_fixture(),
    operation=("imre", "string.capwords"),
)
```

Both this case and `ModuleTest.test_attrs` pass at O0 and O3 with the shared
assertion, as does `test.test_fnmatch.FilterTestCase.test_filter` with its
original imports. All three use native bodies, assertions and module operations
with the original CPython lifecycle around the native process: record
`fixture="mixed"` and `native-body-host-fixture`.

### Stored assertions and exception callbacks

Use the same `testcase_fixture()` for methods that save `self.assertEqual`,
`self.assertListEqual`, or `self.assertRaises`. A saved assertion retains its
original case, including later changes to that case's message settings.
`assertRaises` accepts a bound method or a known function, forwards positional
and keyword arguments, and checks the exception from the invocation. An error
while evaluating an argument still escapes the assertion.

For example, the original `TestTemplate.test_invalid_placeholders` saves
`raises = self.assertRaises` then repeatedly invokes `raises(ValueError,
s.substitute, ...)`. Its source prefix is `"from string import Template\n" +
testcase_fixture()`. Pass `operation=("imre", "string.Template.substitute")` to
`native_method`; the original class and module lifecycle remain host-assisted.

### Running a method with native default fixtures

Use `NativeDefaultMethod` for a discovered, directly defined method of a plain
`unittest.TestCase` with default module, class, and instance fixtures. For
example, the original CPython 3.14.5
`test.test_unittest.testmock.testmock.MockTest.test_assert_not_called` calls a
Mock child and then verifies that its second `assert_not_called()` raises
`AssertionError`. The class, assertion, Mock, and fixture run in the same native
process. Discovery and the reference inventory still run on CPython.

```python
from suite_inventory import load_suite
from suite_native import NativeDefaultMethod, Toolchain, run_evidence

test_id = "test.test_unittest.testmock.testmock.MockTest.test_assert_not_called"
source = upstream / "Lib/test/test_unittest/testmock/testmock.py"
entry = NativeDefaultMethod(
    Toolchain(imre, runtime, clang, level), directory, source,
    "764585ec5d54688b483dca510d6fa187a9fc2531",
    ("imre", "unittest.mock.Mock.assert_not_called"),
)
report = run_evidence(load_suite(reference["module"]), {}, reference, level,
                      native_defaults={(test_id, 1): entry})
```

Use a fresh existing directory for each binary and the full pinned reference
from your module. Pass `reachable_only=True` to `Toolchain` to check that mode.
Declare the location of the operation explicitly. For an operation delegated
to a CPython child, pass `("cpython", description)`; use `assertions="mixed"`
if assertions also run in both processes. The native entry alone cannot
establish that a delegated operation or assertion ran in Imre.
If your full reference includes approved skips, pass the same per-occurrence
`exceptions` as in your other reports.

The entry checks the Git blob, the discovered method's code, and the original
import bindings. It preserves the class header and the complete method bytes,
then invokes native `unittest.main()` with `-v`. A pass requires exit 0, empty
stdout, the exact staged ID, one test, and the success summary on stderr. Only
elapsed time is normalized. The receipt records the original ID and occurrence,
the staged `__main__` ID, source blob, method and entry hashes, mode, streams,
and status. The package itself has not been imported natively by this path.

A decorated method or class, custom setup/teardown, registered cleanup,
configured instance, additional base, class helper or unresolved global needs
its actual fixture. Use the host fixture path when applicable and record its
role. A native compile blocker or unsuccessful run never earns pass credit.

### Matching an exception message

`with case.assertRaisesRegex(ErrorType, pattern) as caught` first checks the
exception class, then searches `str(caught.exception)` using the native regex
engine. The pattern can match anywhere. Use `^` and `$` to anchor it. A
different exception escapes unchanged. The context retains a matching exception
even when its message fails the regex; that failure prints the original pattern
spelling and the exception text, subject to the case's `msg`, `longMessage`
and `failureException` settings.

Literals, runtime strings and already compiled native str patterns use the same
supported grammar. A compiled pattern retains its flags when passed through a
function. Unsupported runtime syntax raises an explicit native boundary error;
bytes patterns and the callable overload remain separate compiler boundaries.
The [exception regex control](full/unittest/context_regex.py) covers search,
Unicode whitespace, compiled patterns, missing and wrong exceptions, mismatched
messages, and exception identity.
