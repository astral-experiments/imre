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
from pathlib import Path
from tempfile import TemporaryDirectory
from suite_inventory import load_suite
from suite_native import Toolchain, native_method, run_in_fixtures, testcase_fixture

# Run from benchmarks/cpython_stdlib with the pinned, test-enabled interpreter.
source = Path("full/unittest/upstream/Lib/test/test_unittest/test_result.py").resolve()
test_id = "test.test_unittest.test_result.Test_TestResult.test_stop"
for level in (0, 3):
    tools = Toolchain(Path("/path/to/imre"), Path("/path/to/libimre_runtime.a"),
                      Path("/path/to/clang"), level)
    with TemporaryDirectory() as work:
        execute = native_method(tools, Path(work), source, "Test_TestResult",
                                test_id, testcase_fixture("bool"))
        evidence, failures, skips = run_in_fixtures(
            load_suite("test.test_unittest"), {test_id: execute})
        assert not failures and not skips
        assert evidence[test_id][0]["execution"] == "native-body"
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

For host-driven operations, pass a callback to `run_in_fixtures` instead.
It receives the original case and bound method after setup. Bind only the
operations implemented by Imre, run the original method, and return evidence
of the actual native calls. The shared runner returns per-occurrence evidence,
failures and skips. Check all three and reconcile them with the **complete**
reference denominator. A method using both native and host product operations
is mixed even when its assertions pass. A method with no native operation earns
no native credit.
