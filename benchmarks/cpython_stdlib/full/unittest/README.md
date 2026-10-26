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

[Identity assertions across upstream suites](identity-assertions.md) lists the exact before/after IDs, execution roles and the command that regenerates their full inventories.

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

Ten `TestLongMessage` methods run under the original CPython class and setup.
Each tests all four combinations of long/short diagnostics and omitted/explicit
messages. The bound operation executes in a native child, which returns the
actual assertion failure and its message. The original helper checks those
messages on the host. For `testAssertRaises`, the original host callback returns
normally before Imre performs the missing-exception check. These ten are
host-driven native operations.

`testAssertDictEqual` passes its original dictionaries to the native assertion
four times; its unchanged host helper checks the returned failure messages.
The [dictionary control](mapping_protocol.py) separately checks native equality
for differently ordered mappings, real mismatches, custom failure classes,
truncated differences, and long Unicode values.

Five original `Test_TestCase` methods run their entire bodies and native
assertions: `testAssertMultiLineEqual`, `testAssertEqualSingleLine`,
`testAssertEqualwithEmptyString`, `testAssertEqualMultipleLinesMissingNewlineTerminator`,
and `testAssertEqualMultipleLinesMismatchedNewlinesTerminators`. Each forces
an actual mismatch and checks the diff. The [equality control](equality_protocol.py)
compares complete failure messages, including first differing and extra list
elements, Unicode, truncation, custom messages, and a subclass's failure class.
It adds no upstream IDs.

At each of O0 and O3: 17 fully native passes, 11 native bodies with host
fixtures, 12 host-driven native operations (48 calls), 23 compile blockers,
6 fixture blockers, 1,019 reference-only and 2 reference skips. The [versioned evidence reports](native-linux.json) retain
all 2,180 records and their [field definitions](../../NATIVE_EVIDENCE.md).

The [native loader example](NATIVE_LOADER.md) exercises the same original
constructor and ten original method-discovery callers with native fixtures: six pass and five have recorded blockers.

The [patch.dict audit](PATCH_DICT.md) records the original fixtures and compiler
results for all 18 methods that call `patch.dict` directly.

Three additional complete equality methods are attempted at each level.
`testAssertMultiLineEqualTruncates` constructs a TestCase with a method name;
`testAssertEqual_diffThreshold` and `testAssertEqual_shorten` both reach
`addCleanup`. Each record retains the original passing reference,
the native diagnostic, source line, and method hash.

The [callable assertion control](callable_protocol.py) verifies that only the
actual invocation belongs inside the catch: the case, bound receiver and
forwarded arguments run first. It covers argument errors, noncallables,
lambda names, a zero-argument lambda that reads local variables, an empty
tuple, custom failure classes and the identity of a mismatched exception.
Two full original callable methods are also attempted:
`test_case.Test_TestCase.testAssertRaisesCallable` still needs a locally
declared exception class and `test_assertions.Test_Assertions.test_assertRaises`
needs an unannotated local function. Their complete methods remain in the report.

The [stored assertion control](bound_assertions.py) exercises aliases, including
one kept in a list and one invoked as another assertion's callback. The
[binding control](callable_binding.py) checks missing and excess arguments,
bound and unbound methods, duplicate arguments, and exceptions raised while
evaluating operands. Both compare actual messages and side effects with the
pinned interpreter and add no upstream IDs.

The [context control](context_protocol.py) compares caught errors, stored managers, argument order and early exits with CPython at both levels. It adds no upstream IDs.

## Exception-family integration controls

`run.py` compares the shipped context protocol against pinned CPython at
both optimization levels. It exercises nested mismatched handlers and the
identity of the original caught object for LookupError, NameError,
UnicodeEncodeError and shutil's OSError subclasses. The default gate also checks
the identity, name and path of [structured import errors](https://github.com/astral-sh/imre/pull/590)
and [csv.Error](https://github.com/astral-sh/imre/pull/468) against unrelated handlers.
These controls must compile, run, and match the reference; they contribute no upstream IDs.

## Native mock calls

The unchanged [MockTest methods](upstream/Lib/test/test_unittest/testmock/testmock.py)
`test_assert_not_called`, `test_assert_called` and `test_assert_called_once`
exercise repeated child lookup, call counts, and positive and negative assertion outcomes.
`test_ordered_call_signature` uses the inherited string assertion to verify the
representation of the recorded call, including keyword order. The [mock control](mock_protocol.py) checks
call order, keyword order, nested return values, exception side effects, and complete
failure text. The original method, recording, assertions, and default fixture
lifecycle execute in the native binary. CPython supplies discovery and the reference.

`import unittest.mock`, `from unittest import mock`, and
`from unittest.mock import Mock` expose the same native implementation.
For example, `stream.write(b"data")` followed by
`stream.write.assert_called_once_with(b"data")` records and checks the actual
argument. This is useful independently of installing a patch. The current recorder
accepts None, bool, int, str, and bytes; an unsupported argument is a compile
diagnostic. `assert_any_call` searches the complete argument history for that mock;
`assert_called_with` checks only its last call. The [reset control](mock_reset.py)
checks both decisions after a reset, previously held call lists, child identity,
and the different reset rules for children and cached return values. The mock
tree can keep an exception instance as its `side_effect`.

A Mock stored in an imported module records calls during module initialization,
through an imported alias, and through the module attribute. The [import control](mock_globals/case.py)
checks that all three calls share one history and compares the failed
`assert_called_once()` message with CPython.

`test_assert_any_call`, `test_assert_called_with` and
`test_assert_called_once_with` exercise the callable form of
`TestCase.assertRaises` on real Mock methods. The last also calls a lambda
that reads its local mock after a reset. These three complete methods use
the same [native default fixture entry](../../SUITE_RUNNER.md#running-a-method-with-native-default-fixtures)
as the four above. Each receipt includes its verbose case ID, one-test summary,
source blob, and complete method hash.

`test_assert_called_once_message_not_called` checks the actual assertion message
from a mock that was never called. The [membership control](membership.py)
also exercises substring, list and dictionary membership, failure messages,
operand evaluation order, and subclass overrides.

Four additional original Mock methods have measured compile blockers.
The three reset methods need constructor
arguments (`parent` or `return_value`) or assign a mock as its own return value.
`test_assert_called_exception_message` also reaches positional `str.format` at
upstream `testmock.py:533`.
The evidence records retain their original CPython passes, exact source lines,
method hashes, and each native diagnostic. These are ordinary blockers.

The subprocess and shutil owners retain their original suite evidence:
an imported mock or an assertion control does not verify their product operations.

The [default-fixture audit](DEFAULT_FIXTURE_AUDIT.md) records what happens when
the shared entry attempts every original occurrence with the full CPython test
package available. Each record identifies the native outcome or the first
fixture or compiler blocker for that entry.

`assertIsNone(obj, msg=None)` and `assertIsNotNone(obj, msg=None)` test
identity with `None`. False, zero, empty strings and the string `"None"` are
distinct from it. The ordinary native failure path preserves `longMessage`
and the case's `failureException`. The [control](none_assertions.py) also
checks optional values, bytes, tuples, and argument evaluation order.
Native values that cannot bind to the compiler's `object` parameter, such as
a list, currently produce a compiler diagnostic.

In the [ordinary and reachable O0/O3 run](https://github.com/astral-sh/imre/actions/runs/40221013955),
`Test_TestCase.testAssertIsNone` executed its unchanged body, assertions and
bound callbacks in Imre with the original CPython fixture. The unchanged
`TestLongMessage.testAssertIsNone` and `testAssertIsNotNone` exercised four
native operations each; CPython ran their fixtures and checked the eight
failure messages. These are three host-assisted cases in the 1,090-occurrence
report.
