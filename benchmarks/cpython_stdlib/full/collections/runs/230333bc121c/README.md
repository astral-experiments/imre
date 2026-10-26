# Collections doctest discovery

CPython 3.14.5 discovers 15 test cases containing 65 examples in
`Lib/collections/__init__.py`. The native catalog stores their original
documented text, covered by the [CPython license](../../../../../../crates/imre-compiler/stdlib/docstrings/LICENSE).
The [probe](../../sealed_doctests.py) verifies the upstream file blobs and
compares the saved text with each actual CPython finder result.

[Before](before.json), the collections module was rejected as the argument
to `DocTestSuite`. [After](after.json), the native suite yields the same 15
IDs in the same order; the docstrings and the six example fields (source,
expected output, exception, options, line and indentation) match all 10,949
bytes of the reference output. The four existing itertools and JSON
discovery controls also match. Each comparison ran in whole and reachable
modes at O0 and O3; none executes CPython inside the native program.

Both the original `Counter.elements` case and the whole suite are still
blocked by its third example, `import math`. Discovery and metadata do
not promote any of the 659 occurrences to a test pass.

Reproduce with a test-enabled CPython 3.14.5 and a compiler and runtime
built from the revision recorded in the report:

```sh
python3.14 -B benchmarks/cpython_stdlib/full/collections/sealed_doctests.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --compiler-revision REV --result discovery.json --require-discovery
```
