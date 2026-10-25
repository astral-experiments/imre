# CPython 3.14.5 JSON compatibility

The [reference run](reference-linux.json) records **216 executions, 212 unique test IDs, and 114 source routines** (112 unittest methods and two doctests). The original [package](../../upstream/Lib/test/test_json/) and [license](../../upstream/LICENSE) are already vendored. Their Git blobs and release commit are in the [upstream manifest](../../UPSTREAM.json). The reference check also verifies the release sources of the doctests.

The package's `load_tests` adds two doctests and two backend identity tests. Package discovery visits each of these IDs twice. Most other methods are inherited by both the Python and C backend fixtures; the command-line tests are inherited by both `TestMain` and `TestTool`. Each entry in the report names the defining source routine, all its concrete IDs, and every occurrence of each ID.

[JSON's doctest fixtures](DOCTEST.md) list the three `DocTestSuite` calls, their 34 distinct examples, and the shared import blocking the intact package runner.

Reproduce with a test-enabled build of the pinned release:

```sh
python -B -m test --single-process --tempdir /tmp --randseed 1 -u cpu \
    --junit-xml /tmp/json-reference.xml test_json
python -B benchmarks/cpython_stdlib/full/json/reference.py /tmp/json-reference.xml \
    --result /tmp/json-reference-linux.json
```

On Linux, 215 executions passed. `TestCEncodeBasestringAscii.test_overflow` has CPython's address-space skip; the resource reason is recorded verbatim. Enabling the CPU resource runs both deeply nested encoder fixtures. These are reference outcomes only. Imre has one native implementation; neither CPython backend establishes a native pass.

## Native calls with host assertions

```sh
python -B benchmarks/cpython_stdlib/full/json/native.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-native.json
```

The [native report](native-linux.json) records both `-O0` and `-O3`. The [shared suite runner](../../SUITE_RUNNER.md) runs the original unittest fixtures, methods, and assertions on CPython; each JSON call runs in the compiled Imre driver. The host transports operands and reconstructs native values for the assertions. Both backend fixtures use the same executable. Each level ran 62 IDs (31 source methods; 356 native calls). Twenty-one binary stream controls and two additional checks of all five native `JSONDecodeError` fields do not increase the upstream test count.

## Compiled upstream methods

Run `bodies.py` with the same options as `native.py`. It uses the [shared suite runner](../../SUITE_RUNNER.md) to compile each complete verified method into a typed native fixture. The original CPython module, class, and instance fixtures run on the host. The child process executes the unmodified body, its assertions, and standard JSON operations in Imre. Both CPython fixture IDs call the same native executable. The [body report](bodies-linux.json) records per-ID execution evidence and source hashes at both levels. Each level passed 22 fixture executions from 11 methods, including all 55,295 iterations of the Unicode escape sweep in each fixture. Ten of the 11 sources also have host-driven evidence above.
