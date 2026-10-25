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

## Native calls with host assertions

```sh
python -B benchmarks/cpython_stdlib/full/json/native.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-native.json
```

The [native report](native-linux.json) records both `-O0` and `-O3`. The original unittest methods run on CPython; each JSON call runs in the compiled Imre driver. The host transports operands and reconstructs native values for the assertions. Both backend fixtures use the same executable. Each level ran 54 IDs (27 source methods; 256 native calls). Sixteen additional binary stream controls check native `json.load` across encodings and malformed code units; these do not increase the upstream test count.
