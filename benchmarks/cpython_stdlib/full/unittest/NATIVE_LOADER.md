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

The [receipt](loader-names-linux.json.gz) records the CPython and Imre
transcripts in ordinary and reachable modes at O0 and O3. It covers the constructor and the seven methods named for `getTestCaseNames`. Six of those seven define a local class; one defines an unannotated local comparator. The receipt records their original lines, hashes and diagnostics. Three additional original methods also call this API. [The local class follow-up](https://github.com/astral-sh/imre/pull/1169) tracks all ten direct callers.

The original `Test_TestLoader.test___init__` now runs once in the primary [1,090-occurrence report](native-linux.json). Its method, assertions, operation and default fixture all run in Imre, [verified in all four configurations](https://github.com/astral-sh/imre/actions/runs/40216305961) on `2df85379543f3eb67b275f15d652e549241badc0`.

The complete module startup in [#578](https://github.com/astral-sh/imre/pull/578)
requires more than class name discovery. Here `TestSuite` and `TextTestRunner`
cannot be imported; storing the imported `test.dis_module` as a value fails
at the assignment; calling `defaultTestLoader.loadTestsFromModule` fails at
method lookup. Each diagnostic is recorded separately. [The module-value PR](https://github.com/astral-sh/imre/pull/1136) independently runs that assignment in all four configurations.

```sh
gzip -dc loader-names-linux.json.gz | jq '.modes["reachable-O3"] | {control, records, startup}'
```
