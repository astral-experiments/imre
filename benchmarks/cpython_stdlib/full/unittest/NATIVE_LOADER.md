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
transcripts in normal and reachable modes at O0 and O3. It also attempts all
eight original constructor and `getTestCaseNames` cases. `Test_TestLoader.test___init__`
passes with native body, assertions, `TestLoader` operation and default fixture.
The primary [1,090-occurrence report](native-linux.json) runs the same method
with a host fixture. The seven others are compiler failures: six define a local
class and one defines an unannotated local comparator. Each has its upstream
line, method hash and diagnostic in the receipt.

The complete module startup in [#578](https://github.com/astral-sh/imre/pull/578)
requires more than class name discovery. Here `TestSuite` and `TextTestRunner`
cannot be imported; storing the imported `test.dis_module` as a value fails
at the assignment; calling `defaultTestLoader.loadTestsFromModule` fails at
method lookup. Each of the four diagnostics is recorded separately.

```sh
gzip -dc loader-names-linux.json.gz | jq '.modes["reachable-O3"] | {control, records, startup}'
```
