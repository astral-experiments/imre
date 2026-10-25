# Running pinned CPython methods in Imre

Import `suite_inventory` and `suite_native` from this directory. Use the release's
test-enabled Python for discovery, shared fixtures and host assertions. Build each
native entry at both `Toolchain(..., level=0)` and `level=3`.
Use `Toolchain(..., level=level, reachable_only=True)` for a second pair with
the compiler's reachability filter. The unittest gate accepts `--reachable-only`;
give its second invocation a separate `--result` path.

## What discovery runs where

`suite_inventory.load_suite()` asks the pinned CPython loader to discover the
original IDs, including package `load_tests` hooks. `run_evidence()` can
substitute a compiled body or operation under those host fixtures. Neither
operation verifies the public native `unittest` loader.

The [startup witness](https://github.com/astral-sh/imre/pull/578) separates an
upstream module import from invoking a standard driver. Use the
[object probes](full/unittest/runner_objects.py) to split that driver further.
Each control builds **and runs** at O0/O3 in ordinary and reachable mode.

| Control | What a successful result establishes |
| --- | --- |
| Import `unittest`, `TestCase`, `TestResult`, `TestLoader`, `defaultTestLoader`, `TestSuite`, `TextTestRunner`, or `mock` separately | That exact public name resolves. |
| Import and invoke `test.dis_module.f` | The verified upstream module can be imported without a loader. |
| Store the imported module; pass it to a function | Native module values work in that particular operation. |
| Collect `test.dis_module` | A real module with no cases yields zero. |
| Collect and run upstream `bar.test_bar` | A real module containing `PassingTest.test_true` yields and runs one case. |
| Run the observable pass/fail fixture | Both methods run in loader order, their setup and teardown run, and the result reports two tests and a failure. |
| Run nested suites and an overridden `failureException` | Selecting a case by method name, unwinding a failing child, and assigning failures to the concrete case's exception class work independently of discovery. |
| Run cases with different `run` signatures | An explicit suite calls a valid override and propagates a real binding error for an invalid one. |

An empty run does not demonstrate nonempty discovery. The controls record the
stage, compiler digest and diagnostics and confer no additional upstream test
IDs. Package recursion, `load_tests`, duplicates, fixture outcomes, text output,
and protocol overrides require their own evidence. Each executable records stdout
separately from `matches_reference_stderr`. In particular, the observable
failure currently lacks CPython's traceback. Its unsuccessful status and test
count are still observable. Normal package import has its own import chain:
the coordinator's guard/ImportError witness at `03977539` reached
`test/support/__init__.py:6` (`annotationlib`) before native discovery.

### The public native loader

The [eight-line example](full/unittest/native_loader_example.py) passes an imported
module to `unittest.defaultTestLoader.loadTestsFromModule`, wraps the discovered
cases in a `TestSuite` and passes it to `TextTestRunner(verbosity=2).run`.
It prints the discovered count, the number actually run, and the success flag
on separate lines: `1`, `1`, `True`. The verbose result names the original
`bar.test_bar.PassingTest.test_true`; its body calls the native `assertTrue`.

From the repository root, run that exact file and the unmodified upstream
fixture in ordinary and reachable mode, each at O0 and O3:

```sh
"$CPYTHON" -B benchmarks/cpython_stdlib/full/unittest/runner_objects.py \
    --cpython-source "$CPYTHON_SOURCE" --imre "$IMRE" --runtime "$IMRE_RUNTIME" \
    --clang "$CLANG" --probe run:upstream --require-match --result /tmp/native-loader-example.json
```

The [Linux example receipt](full/unittest/native-loader-example-linux.json)
includes the source, compiler, and runtime checksums and each output comparison.
The object probe checks the fixture's upstream Git blob before executing it.
CPython provides a separate reference run and does not execute setup, the test
body, its assertion, or teardown for the native executable. This discovery
control adds no cases to the 1,090-occurrence unittest inventory.
The [failure receipt](full/unittest/native-loader-failure-linux.json) shows
that two setup/body/teardown sequences ran and the result was unsuccessful.
Its status and stdout match; its traceback on stderr does not. The same
`--require-match` gate exits with status 1 on this control.
Substitute `test.dis_module` to exercise the zero-test case. The native loader
accepts a static imported module value and discovers sorted fixed TestCase
classes and methods. Package `load_tests`, dynamic module namespaces and class
or module fixture hooks currently report a diagnostic. The executor preserves
case setup, method invocation, teardown, result state and totals after cleanup.

### A native fixture in evidence v1

For an unchanged, undecorated method with default TestCase lifecycle, pass
`native_fixture_method` to `run_evidence` by exact ID and occurrence:

```python
from suite_native import native_fixture_method

test_id = "test.test_unittest.test_suite.Test_TestSuite.test_init__empty_tests"
program = native_fixture_method(
    tools, Path(work), root / "upstream/Lib/test/test_unittest/test_suite.py",
    "Test_TestSuite", test_id, operation="unittest.TestSuite.run and countTestCases",
)
report = run_evidence(load_suite("test.test_unittest"), {}, reference, level,
                      native_fixtures={(test_id, 1): program}, exceptions=reference_skips(level))
```

The adapter compiles the pinned method bytes inside a selected class under
its original module name. It verifies the exact ID reported by the native result.
CPython inventories the full suite and launches the executable. It runs none of the
four measured roles for that occurrence. A custom upstream lifecycle, decorator
or registered cleanup requires an explicit fixture. Whole-module import and
package initializer behavior are measured separately. The result frame records
each native error, failure or skip with its full reason; output from the test
is kept separate from that frame.

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
Use the default fixture for the supported `TestCase` methods, including
boolean and mixed boolean/integer equality. Module owners may provide their
actual typed instance fixture.
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

The [textwrap acceptance driver](full/unittest/textwrap_exception.py) verifies
the upstream test blob and runs both original `DedentTestCase.test_type_error`
contexts in the native loader. Its normal and reachable invocations each
produce two 68-occurrence reports. The other 67 rows must equal the textwrap
owner's corresponding baseline, including their fixture and assertion roles.
CI supplies its freshly generated ordinary baseline; a standalone invocation
generates one when `--baseline` is omitted.
