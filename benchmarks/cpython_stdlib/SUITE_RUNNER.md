# Running pinned CPython methods in Imre

Import `suite_inventory` and `suite_native` from this directory. Use the release's
test-enabled Python for discovery, shared fixtures and host assertions. Build each
native entry at both `Toolchain(..., level=0)` and `level=3`.

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
                                testcase_fixture("bool"), operation=("imre", "unittest.TestResult.stop"))
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
Use the default fixture for the supported `TestCase` methods; the boolean
equality fixture is shared here while the compiler requires a single parameter
type per method. Module owners may provide their actual typed instance fixture.
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
    "unittest.mock.Mock.assert_not_called",
)
report = run_evidence(load_suite(reference["module"]), {}, reference, level,
                      native_defaults={(test_id, 1): entry})
```

Use a fresh existing directory for each binary and the full pinned reference
from your module. Pass `reachable_only=True` to `Toolchain` to check that mode.
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
