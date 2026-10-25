# CPython 3.14.5 JSON compatibility

The [reference run](reference-linux.json) records **216 executions, 212 unique test IDs, and 114 source routines** (112 unittest methods and two doctests). The original [package](../../upstream/Lib/test/test_json/) and [license](../../upstream/LICENSE) are already vendored. Their Git blobs and release commit are in the [upstream manifest](../../UPSTREAM.json). The reference check also verifies the release sources of the doctests.

The package's `load_tests` adds two doctests and two backend identity tests. Package discovery visits each of these IDs twice. Most other methods are inherited by both the Python and C backend fixtures; the command-line tests are inherited by both `TestMain` and `TestTool`. Each entry in the report names the defining source routine, all its concrete IDs, and every occurrence of each ID.

Reproduce with a test-enabled build of the pinned release:

```sh
python -B -m test --single-process --tempdir /tmp --randseed 1 -u cpu \
    --junit-xml /tmp/json-reference.xml test_json
python -B benchmarks/cpython_stdlib/full/json/reference.py /tmp/json-reference.xml \
    --result /tmp/json-reference-linux.json
```

On Linux, 215 executions passed. `TestCEncodeBasestringAscii.test_overflow` has CPython's address-space skip; the resource reason is recorded verbatim. Enabling the CPU resource runs both deeply nested encoder fixtures. These are reference outcomes only. Imre has one native implementation; neither CPython backend establishes a native pass.
