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

The [v1 report](evidence-linux.json) covers all 216 occurrences at each optimization level, including the package submodules, both backend fixtures and both CLI variants. With the iterative parser and reusable decoder, 12 complete native methods ran under 24 host fixture IDs; 44 additional fixture IDs called native operations from host methods. The other 148 occurrences are 98 blocked, 38 reference-only, 11 individually reviewed backend/private API exclusions, and the single upstream skip. The report includes partial calls, native key owners, exact recursion exceptions, concrete diagnostics and source locations.

```sh
python -B benchmarks/cpython_stdlib/full/json/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/json/reference-linux.json /tmp/json-evidence.json
```

The [doctest handoff](DOCTEST.md) identifies the first shared intact-package import blocker.

The [first key identity rerun](key-identity-linux.json) distinguished native key owners from the strings reconstructed by the repr transport and reached the missing `JSONDecoder` constructor. With a [live native decoder](decoder-keys-linux.json), both original `test_keys_reuse` fixture IDs complete at O0 and O3. Each executes `json.loads`, constructs a native `json.decoder.JSONDecoder`, calls its `decode` and reads the same instance's `memo`. The host preserves the observed native owners for the original `sorted` and identity assertions. The ordinary repr transport still fails the first assertion. Run `key_identity.py` with the same options as `evidence.py` to reproduce both paths.

The [deep decoder rerun](deep-decoder-linux.json) records both original `test_highly_nested_objects_decoding` fixture IDs. At O0 and O3 all six native loads per level raised `RecursionError` for the original 500,000-level documents. The method, decorators, recursion context and assertions ran on CPython; only the loads ran in Imre. The report retains Imre's exception text alongside CPython's C and pure reference messages. Separate controls traverse accepted deep values, compare equality and repr through 50,000 levels, and compare all five `JSONDecodeError` fields on malformed inputs. Run `deep_decode.py` with the same command-line options as `evidence.py`.

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

### The three round-trip failures

The [diagnostic run](roundtrips-linux.json) isolates `test_pass1`, `test_pass2` and `test_pass3`. Each defines one `test_parse` method inherited by a Python and a C fixture. At both optimization levels the native parse–serialize–parse sequence produced the same output bytes as the pinned reference for all three inputs. The final inherited `TestCase.assertEqual` raised `TypeError: expected int, got list from JSON` for the first two and `TypeError: expected int, got dict from JSON` for the third.

The [reproducer](roundtrips.py) also compiles the unchanged methods with just `assertEqual` overridden to compare its operands directly. On the measured revision, the compiler rejected comparisons between two parsed JSON values. Three independent controls change a nested leaf and require the native assertion to reject the unequal documents. These controls exercise the assertion; they add no upstream occurrences. The fixture lifecycle remains on CPython in both attempts.

```sh
python -B benchmarks/cpython_stdlib/full/json/roundtrips.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --imre-revision "$(git rev-parse HEAD)" --result /tmp/json-roundtrips.json
```
