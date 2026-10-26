# Shared TestCase identity assertions

The pinned CPython 3.14.5 default suites discover 31 occurrences in `test.test_bool` and 328 in `test.test_patma`. All 359 pass on reference Linux. With the shared native assertions, eight of these original methods now pass in the default native TestCase lifecycle. The remaining 351 are reference-only in this measurement. No test body, operand, expected result, or fixture was changed.

| Upstream ID (occurrence 1) | Before: missing native method at | After: ordinary / reachable, O0 / O3 |
| --- | --- | --- |
| `test.test_bool.BoolTest.test_int` | `case.py:6:9` | fully native in all four |
| `test.test_bool.BoolTest.test_isinstance` | `case.py:5:9` | fully native in all four |
| `test.test_bool.BoolTest.test_issubclass` | `case.py:5:9` | fully native in all four |
| `test.test_patma.TestPatma.test_patma_003` | `case.py:9:9` | fully native in all four |
| `test.test_patma.TestPatma.test_patma_004` | `case.py:9:9` | fully native in all four |
| `test.test_patma.TestPatma.test_patma_005` | `case.py:9:9` | fully native in all four |
| `test.test_patma.TestPatma.test_patma_006` | `case.py:9:9` | fully native in all four |
| `test.test_patma.TestPatma.test_patma_007` | `case.py:9:9` | fully native in all four |

The failure locations were reproduced on [Imre `665f80b9`](https://github.com/astral-sh/imre/commit/665f80b94ff4853b343f2065cce6e3e0bf7e71a1). The [measured compiler change](https://github.com/astral-sh/imre/commit/4649faca7189f00c4b8e579a1b7a4233ed555ad1) changes 32 observations across the four configurations; all 1,404 other observations are identical.

The shared [runner](../../suite_native.py) compiles the checksum-verified method with its original imports, then runs `unittest.main()` in a native child and checks the verbose test ID, result and process status. All four roles in these eight observations (body, assertions, operation and fixture) are `imre`. CPython discovery and the independent regrtest reference run do not execute the measured native operations.

## Reproduce

Run with a test-enabled CPython 3.14.5, the source for [commit 5607950](https://github.com/python/cpython/commit/5607950ef232dad16d75c0cf53101d9649d89115), and a matching Imre compiler and runtime. The script checks the upstream license, the Git blobs of both test modules, and the interpreter's original test files. It matches every discovered occurrence to regrtest's XML before running a selected method.

```sh
"$PYTHON" -E -B benchmarks/cpython_stdlib/full/unittest/identity_cases.py \
    --cpython-source "$CPYTHON_SOURCE" --imre "$IMRE" --runtime "$RUNTIME" \
    --clang "$CLANG" --output "$EVIDENCE"
```

Use `--expect unavailable` with the parent compiler to check each original failure location. Both runs write full-denominator [v1 evidence reports](../../suite_evidence.py) for ordinary and reachable builds, O0 and O3, including the original method hash and the roles for every occurrence.

## Other consumers

- `test.test_unittest.test_case.Test_TestCase.testAssertIs` and `testAssertIsNot` exercise successful and deliberately failing comparisons, including bound assertions passed to `assertRaises`. Their original CPython class and instance fixtures run outside the native child, so these are **native body with host-assisted fixture**.
- `test.test_copy.TestCopy.test_deepcopy_empty_tuple` can use the shared assertion with its native default fixture. The copy owner's suite already counted this occurrence as fully native with a dedicated assertion; it does not add a copy pass.
- The directly discovered `test.datetimetester.TestModule.test_utc_alias` runs with a native default fixture. The full datetime suite instead discovers `test.datetimetester.TestModule_Pure.test_utc_alias` and `test.datetimetester.TestModule_Fast.test_utc_alias`; their original class fixtures replace and restore CPython's imported modules. Under that suite both occurrences have **host-assisted fixtures**.
