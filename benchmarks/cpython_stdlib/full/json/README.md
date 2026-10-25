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

## Full native evidence

The [v1 report](evidence-linux.json) covers the entire package at each optimization level. There is one package denominator, including its submodules, the two backend fixtures, and both CLI variants. It was exercised on Imre `7630556901c9936c9ca112f7fb2f4e52f3706fd3`. At each level, 11 complete native method bodies ran under 22 host fixture IDs; 34 additional fixture IDs called native operations from host methods. The remaining 160 occurrences are 98 blocked, 12 failed, 38 reference-only, 11 individually reviewed backend/private API exclusions, and the single upstream skip. The report includes partial calls, concrete diagnostics and source locations.

```sh
python -B benchmarks/cpython_stdlib/full/json/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/json/reference-linux.json /tmp/json-evidence.json
```

The 12 failed fixture occurrences cover key identity, structured error fields and excessive nesting. The [doctest handoff](DOCTEST.md) identifies the first shared intact-package import blocker. Native results from other branches enter this report after their product changes land and the fixtures run again.

## Native calls with host assertions

```sh
python -B benchmarks/cpython_stdlib/full/json/native.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-native.json
```

The [native report](native-linux.json) records both `-O0` and `-O3`. The original unittest methods run on CPython; each JSON call runs in the compiled Imre driver. The host transports operands and reconstructs native values for the assertions. Both backend fixtures use the same executable. Each level ran 54 IDs (27 source methods; 256 native calls). Twenty-one additional binary stream controls check native `json.load` across encodings, doubled byte order marks and malformed code units; these do not increase the upstream test count.

## Compiled upstream methods

Run `bodies.py` with the same options as `native.py`. It uses the [shared suite runner](../../SUITE_RUNNER.md) to compile each complete verified method into a typed native fixture. The original CPython module, class, and instance fixtures run on the host. The child process executes the unmodified body, its assertions, and standard JSON operations in Imre. Both CPython fixture IDs call the same native executable. The [body report](bodies-linux.json) records per-ID execution evidence and source hashes at both levels. Each level passed 22 fixture executions from 11 methods, including all 55,295 iterations of the Unicode escape sweep in each fixture. Ten of the 11 sources also have host-driven evidence above.
