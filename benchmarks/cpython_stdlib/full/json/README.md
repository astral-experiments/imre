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

The [v1 report](evidence-linux.json) covers the entire package at each optimization level. There is one package denominator, including its submodules, the two backend fixtures, and both CLI variants. It was exercised on Imre `6bafa669b5af591bfe855168f28505fdac4324dd`. At each level, 12 complete native methods ran under 24 host fixture IDs; 40 additional fixture IDs called native operations from host methods. The remaining 152 occurrences are 98 blocked, four failed, 38 reference-only, 11 individually reviewed backend/private API exclusions, and the single upstream skip. The report includes partial calls, concrete diagnostics and source locations.

```sh
python -B benchmarks/cpython_stdlib/full/json/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/json/reference-linux.json /tmp/json-evidence.json
```

The four failed fixture occurrences cover key identity and excessive nesting. The [doctest handoff](DOCTEST.md) identifies the first shared intact-package import blocker.

## Native calls with host assertions

```sh
python -B benchmarks/cpython_stdlib/full/json/native.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-native.json
```

The [native report](native-linux.json) records both `-O0` and `-O3`. The [shared suite runner](../../SUITE_RUNNER.md) runs the original unittest fixtures, methods, and assertions on CPython; each JSON call runs in the compiled Imre driver. The host transports operands and reconstructs native values for the assertions. Both backend fixtures use the same executable. Each level ran 62 IDs (31 source methods; 356 native calls). Twenty-one binary stream controls and two additional checks of all five native `JSONDecodeError` fields do not increase the upstream test count.

## Compiled upstream methods

Run `bodies.py` with the same options as `native.py`. It uses the [shared suite runner](../../SUITE_RUNNER.md) to compile each complete verified method into a native fixture. The original CPython module, class, and instance fixtures run on the host. The child process executes the unmodified body, its assertions, and standard JSON operations in Imre. Both CPython fixture IDs call the same native executable. The [body report](bodies-linux.json) records per-ID execution evidence and source hashes at both levels. Each level passed 24 fixture executions from 12 methods. Each fixture ran all 55,295 iterations of the Unicode escape sweep, and all 34 inputs in the JSON checker failure test.

Run `sweep.py` with the same flags to try every one of the 112 defining unittest methods with the native TestCase and the pinned module declarations and class helpers it references. The [sweep](routine-sweep-linux.json) records the outcome for each definition and its fixture IDs at O0 and O3. With this general fixture, two methods completed, three compiled but failed at runtime, 106 had compile or decorator blockers, and one retained its upstream resource skip. These are alternate attempts at the same occurrences, not an additional denominator. The typed fixture checks above cover another ten methods. For example, the general integer assertion projects the decoded results of the three `test_parse` round trips to int; it fails when those results are lists or dictionaries.
