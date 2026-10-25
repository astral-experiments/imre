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

The [v1 report](evidence-linux.json) covers all 216 occurrences at each optimization level. Eighteen complete native methods ran under 36 host fixture IDs. Another 47 fixture IDs called native operations under host methods and assertions. The remaining 133 occurrences are 83 blocked, 38 reference-only, 11 reviewed backend or private API exclusions, and one original CPython skip. Both decoder fixtures use the same native implementation.

The [hosted decoder run](https://github.com/astral-sh/imre/actions/runs/40179030429) repeated all these measurements at O0 and O3 on source `a3390bbf218f8d0c9354bea3676ba7487f652f06`. It verifies the 216 reference outcomes, saves the compiler and runtime SHA-256 hashes, and retains the complete-method, key identity, nesting, and intact-package reports. The [complete-method sweep](routine-sweep-linux.json) and [round-trip report](roundtrips-linux.json) from [#874](https://github.com/astral-sh/imre/pull/874) retain the earlier measurements.

```sh
python -B benchmarks/cpython_stdlib/full/json/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/json/reference-linux.json /tmp/json-evidence.json
```

The [doctest handoff](DOCTEST.md) identifies the first shared intact-package import blocker.

The [first key identity rerun](key-identity-linux.json) distinguished native key owners from the strings reconstructed by the repr transport and reached the missing `JSONDecoder` constructor. With a [live native decoder](decoder-keys-linux.json), both original `test_keys_reuse` fixture IDs complete at O0 and O3. Each executes `json.loads`, constructs a native `json.decoder.JSONDecoder`, calls its `decode` and reads the same instance's `memo`. The host preserves the observed native owners for the original `sorted` and identity assertions. The ordinary repr transport still fails the first assertion. Compiling the full unchanged method in the general fixture rejects `json = json`. Giving the fixture import a separate name reaches the next blocker: the original helper call passes `self.loads` as a first-class callable. Three compile-only probes isolate passing the module function, passing a bound `decoder.decode`, and sorting a decoded object; none executes a method or contributes a pass. Run `key_identity.py` with the same options as `evidence.py` to reproduce both paths.

The [deep decoder rerun](deep-decoder-linux.json) records both original `test_highly_nested_objects_decoding` fixture IDs. At O0 and O3 all six native loads per level raised `RecursionError` for the original 500,000-level documents. The method, decorators, recursion context and assertions ran on CPython; only the loads ran in Imre. The report retains Imre's exception text alongside CPython's C and pure reference messages. Separate controls traverse accepted deep values, compare equality and repr through 50,000 levels, and compare all five `JSONDecodeError` fields on malformed inputs. Run `deep_decode.py` with the same command-line options as `evidence.py`.

## Native calls with host assertions

```sh
python -B benchmarks/cpython_stdlib/full/json/native.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-native.json
```

The [native report](native-linux.json) records both `-O0` and `-O3`. The [shared suite runner](../../SUITE_RUNNER.md) runs the original unittest fixtures, methods, and assertions on CPython; each JSON call runs in the compiled Imre driver. The host transports operands and reconstructs native values for the assertions. Both backend fixtures use the same executable. Each level ran 62 IDs (31 source methods; 356 native calls). Twenty-one binary stream controls and two additional checks of all five native `JSONDecodeError` fields do not increase the upstream test count.

## Compiled upstream methods

Run `bodies.py` with the same options as `native.py`. The [shared suite runner](../../SUITE_RUNNER.md) compiles each verified method. The original CPython module, class, and instance fixtures run on the host. The child executes the unchanged body, its assertions, and its JSON operations in Imre. Both fixture IDs call the same native executable. The [body report](bodies-linux.json) records 36 fixture executions from 18 methods at each level, including all 55,295 iterations of the Unicode escape sweep, all 34 JSON checker failure inputs, and integer serialization through `2**64`.

Run `sweep.py` with the same flags to try every one of the 112 defining unittest methods with the native TestCase and the pinned declarations and helpers it references. The [general fixture sweep](routine-sweep-linux.json) records 13 completed methods, three runtime failures, 95 compile or decorator blockers, and the original resource skip at each level. These are alternate attempts at the same occurrences. Typed fixtures complete five further methods: two Unicode decoding methods and the three round trips described below.

### The three round-trip failures

The [diagnostic run](roundtrips-linux.json) isolates `test_pass1`, `test_pass2` and `test_pass3`. Each defines one `test_parse` method inherited by a Python and a C fixture. At both optimization levels the native parse–serialize–parse sequence produced the same output bytes as the pinned reference for all three inputs. The final inherited `TestCase.assertEqual` raised `TypeError: expected int, got list from JSON` for the first two and `TypeError: expected int, got dict from JSON` for the third.

The [reproducer](roundtrips.py) also compiles the unchanged methods with just `assertEqual` overridden to compare its parsed operands directly. All three original methods complete under both fixture IDs with this assertion. Three independent controls change a nested leaf; the native assertion rejects every unequal pair. These controls add no upstream occurrences. The fixture lifecycle remains on CPython in both attempts.

```sh
python -B benchmarks/cpython_stdlib/full/json/roundtrips.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --imre-revision "$(git rev-parse HEAD)" --result /tmp/json-roundtrips.json
```
