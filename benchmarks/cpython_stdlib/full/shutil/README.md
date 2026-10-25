# CPython 3.14.5: shutil

The pinned release contains one test module, `Lib/test/test_shutil.py`. Loading it with unittest discovers **223 distinct IDs**, including inherited methods on the bytes and zero-copy classes. The [manifest](UPSTREAM.json) points to the existing byte-for-byte copies of the test and CPython license. The [Linux reference](reference-linux.json) records every ID: 173 pass and 50 skip on this host. Each skip has the reason reported by CPython. These numbers describe the oracle only.

To reproduce the reference with a test-enabled build of the pinned release, use the shared inventory runner from [#479](https://github.com/astral-sh/imre/pull/479):

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/shutil/UPSTREAM.json --result /tmp/shutil-reference.json
```

The runner verifies both the vendored and installed test blobs, discovers the full suite, and runs regrtest in a temporary directory. It requires regrtest's IDs to agree with unittest discovery.

## Native execution

[evidence-linux.json](evidence-linux.json) contains one row per occurrence at `-O0` and `-O3`, including every original skip and its source condition. At this revision, seven complete methods run their bodies and assertions in Imre under CPython fixtures; 33 use host assertions on native lookup, copying, or metadata operations. Two methods also call CPython `shutil.rmtree` from the original body. Both missing-PATH methods pass with mixed execution: the native child sees no PATH, asks the original host mocks for `os.confstr("CS_PATH")` and (only after ValueError) `os.defpath`, and performs the executable search. Each method makes two native lookups at each level. Together with the two cleanup methods, this gives four mixed passes and zero observed failures. The remaining 129 are reference-only; none of the 50 upstream skips is counted as a native pass.

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/full/shutil/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/shutil-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/shutil/reference-linux.json /tmp/shutil-evidence.json
```

The first command writes all outcomes and exits unsuccessfully when an assertion fails. The second checks the report against the full pinned denominator. [mock-fixtures.json](mock-fixtures.json) identifies the original `unittest.mock` call sites, decorators and affected IDs, including those skipped by CPython.

The [complete TestCopy run](copy-linux.json) also checks the real 1 KiB user extended attribute outside the upstream trusted-attribute skip. The return-value test exercises four native calls; its complete-method compile stops on the tuple of shutil function values. It remains host-driven.
