# Discovering method names with `unittest.TestLoader`

With `upstream/Lib/test/test_unittest/namespace_test_pkg` on the module search
path, this uses CPython's [original class](upstream/Lib/test/test_unittest/namespace_test_pkg/bar/test_bar.py):

```python
from bar.test_bar import PassingTest
from unittest import defaultTestLoader

print(defaultTestLoader.getTestCaseNames(PassingTest))  # ['test_true']
```

`TestLoader()` creates a fresh loader with its own `errors` list. For a class
known at compilation, `getTestCaseNames` collects inherited and overridden
callable names, sorts and deduplicates them, then filters by `testMethodPrefix`
and optional `testNamePatterns`. Patterns match the qualified name with
case-sensitive `fnmatchcase`. Discovery never calls a test body or property
getter. The [executable example](loader_names.py) checks all of these, including
that a wrong-case pattern and an empty pattern list select nothing. Empty or
private prefixes and matching unsupported descriptors raise `NotImplementedError`.

### Classes declared inside test methods

After the declaration the loader can inspect a local class's methods, inherited names, and class attributes that shadow them. Its [executable example](loader_local_classes.py) uses the original qualified name, including `<locals>`. [Boundary controls](loader_class_boundaries.py) cover access before the declaration, conditional bindings, rebinding and defaults with side effects.

The [primary report](native-linux.json) attempts the constructor and all ten original `Test_TestLoader` methods that call `getTestCaseNames`. The [four-configuration run](https://github.com/astral-sh/imre/actions/runs/40220036532) used CPython blob `cdff6d1a20c8dfda384178ab48403074e66409bd`. Each outcome matches in ordinary and reachable builds at O0 and O3.

| Original method | Native result |
| --- | --- |
| `test___init__` | Pass: construction and a fresh errors list |
| `test_getTestCaseNames` | Pass: method selection |
| `test_getTestCaseNames__inheritance` | Pass: inherited and overridden names |
| `test_getTestCaseNames__no_tests` | Pass: empty result |
| `test_getTestCaseNames__testNamePatterns` | Pass: qualified name filtering |
| `test_sortTestMethodsUsing__default_value` | Pass: default sort order |
| `test_getTestCaseNames__not_a_TestCase` | Blocked: local class inherits from `int` |
| `test_getTestCaseNames__testNamePatterns__attribute_access_regression` | Blocked: local descriptor initialization |
| `test_partial_functions` | Blocked: unannotated local function used by a partial |
| `test_sortTestMethodsUsing__getTestCaseNames` | Blocked: unannotated local comparator |
| `test_sortTestMethodsUsing__None` | Blocked: assignment to `sortTestMethodsUsing` |

For the six passes, the original body, assertions, loader operation and default fixture all execute on Imre. The other five pass on CPython and remain native compiler failures; their records include the exact diagnostic and method hash. The [earlier module-level receipt](loader-names-linux.json.gz) covers the constructor and the seven methods named for `getTestCaseNames`.

Of the 1,090 occurrences, 13 were fully native, 8 used native bodies with host fixtures, 9 had host bodies calling native operations, 15 were blocked, 1,043 were reference-only, and two had upstream signal skips.

The complete module startup in [#578](https://github.com/astral-sh/imre/pull/578)
requires more than class name discovery. Here `TestSuite` and `TextTestRunner`
cannot be imported; storing the imported `test.dis_module` as a value fails
at the assignment; calling `defaultTestLoader.loadTestsFromModule` fails at
method lookup. Each diagnostic is recorded separately. [The module-value PR](https://github.com/astral-sh/imre/pull/1136) independently runs that assignment in all four configurations.

```sh
gzip -dc loader-names-linux.json.gz | jq '.modes["reachable-O3"] | {control, records, startup}'
```
