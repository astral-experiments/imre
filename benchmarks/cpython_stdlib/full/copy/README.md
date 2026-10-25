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
