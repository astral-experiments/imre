# CPython 3.14.5: copy

The [original test module](upstream/Lib/test/test_copy.py) discovers 81 distinct
test IDs. All 81 passed on the Linux CPython 3.14.5 reference build, including
shallow copy, deep copy, recursive containers, custom copy and pickle protocols,
and replacement. The [reference report](reference-linux.json) records each ID.
These results describe CPython; they do not count as Imre passes.

[UPSTREAM.json](UPSTREAM.json) pins the test, [implementation](upstream/Lib/copy.py)
and Python license to their original Git blobs. To reproduce the reference using
a test-enabled build of the pinned release:

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/copy/UPSTREAM.json --result /tmp/copy-reference.json
```

## Native coverage

Four complete upstream methods execute at both O0 and O3: `test_copy_basic`,
`test_copy_list`, `test_copy_dict`, and `test_copy_set`. Their 13 original
assertions run in native executables with typed equality and identity checks.
The [native report](native-linux.json) retains all 81 IDs; three additional methods run their original assertions on CPython after copying their
instance in a native process. Two supply their byte-identical `__copy__` method;
`test_copy_inst_vanilla` exercises the native default. The other 74 have
reference evidence only. The mixed fixtures transfer one typed integer field
and check the native result class and distinct identity; they do not count as fully native tests. A separate differential fixture exercises shared
children, independent outer containers, and Counter/defaultdict missing keys.
