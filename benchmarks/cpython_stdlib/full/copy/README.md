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

Six complete upstream methods execute at both O0 and O3: shallow copy of an
integer, list, dict and set; deep copy of an integer and the empty tuple.
Their 15 original assertions run in native executables. The [native report](native-linux.json)
retains all 81 IDs. The other 75 have reference evidence only.

Separate differential fixtures exercise shared children, independent outer
containers, Counter/defaultdict missing keys, a tuple pointing into the same
graph as a dictionary, and an optional list. Runtime tests create a list/dict
cycle and a tuple/list cycle directly, check that their back edges point to
the copies, and explicitly break the cycles. The compiler still rejects cyclic
heap types in Python source, so these runtime checks do not count the upstream
reflexive-container tests as native. Caller-supplied memo dictionaries and
custom copy/pickle protocols require separate support.
