# CPython 3.14.5: shutil

The pinned release contains one test module, `Lib/test/test_shutil.py`. Loading it with unittest discovers **223 distinct IDs**, including inherited methods on the bytes and zero-copy classes. The [manifest](UPSTREAM.json) points to the existing byte-for-byte copies of the test and CPython license. The [Linux reference](reference-linux.json) records every ID: 173 pass and 50 skip on this host. Each skip has the reason reported by CPython. These numbers describe the oracle only.

To reproduce the reference with a test-enabled build of the pinned release, use the shared inventory runner from [#479](https://github.com/astral-sh/imre/pull/479):

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/shutil/UPSTREAM.json --result /tmp/shutil-reference.json
```

The runner verifies both the vendored and installed test blobs, discovers the full suite, and runs regrtest in a temporary directory. It requires regrtest's IDs to agree with unittest discovery.

## Native execution

The merged implementation covers [file contents and copy errors (#492)](https://github.com/astral-sh/imre/pull/492), [metadata and extended attributes (#504)](https://github.com/astral-sh/imre/pull/504), and [the missing-PATH defaults for text and bytes (#720)](https://github.com/astral-sh/imre/pull/720). [#522](https://github.com/astral-sh/imre/pull/522) adds the deletion operations measured below.

[evidence-linux.json](evidence-linux.json) contains all 223 occurrences at `-O0` and `-O3`. At each level, seven complete methods run their bodies and assertions in Imre under CPython fixtures, and 41 use host assertions on native lookup, copying, metadata or deletion. Twelve passes are mixed: eight call original Python deletion handlers, two call CPython `shutil.rmtree` during copy cleanup, and two use the original default-search-path mocks. In those two methods the native child has no `PATH`; it requests `os.confstr("CS_PATH")` and, after `ValueError`, `os.defpath`, then searches for the executable. Of the 113 blocked occurrences, three deletion methods need syscall mocks; for the other 110, the first complete-method diagnostic is recorded. The remaining 50 have original upstream skips. There are zero observed failures.

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/full/shutil/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/shutil-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/shutil/reference-linux.json /tmp/shutil-evidence.json
```

The first command writes all outcomes and exits unsuccessfully when an assertion fails. The second checks the report against the full pinned denominator. [mock-fixtures.json](mock-fixtures.json) identifies the original `unittest.mock` call sites, decorators and affected IDs, including those skipped by CPython.

The [complete TestCopy run](copy-linux.json) also checks the real 1 KiB user extended attribute outside the upstream trusted-attribute skip. The return-value test exercises four native calls; its complete-method compile stops on the tuple of shutil function values. It remains host-driven.

The [complete TestRmTree run](rmtree-linux.json) records each handler invocation. The native walker chooses onexc over onerror, reports the failing operation and exception, and resumes after the original handler returns. The Linux test transport supplies the host OS function object and reconstructs the deprecated onerror traceback in CPython. These eight methods are mixed; passing them does not demonstrate that the compiler accepts arbitrary Python callback arguments.

## Complete-method attempts

[method-attempts-linux.json](method-attempts-linux.json) records the compiler and runtime hashes, original method and decorator hashes, and the first diagnostic for each blocked build. All 173 reference passes were attempted under their original CPython fixtures at both levels. Seven native bodies completed; 166 could not compile. Of those 166, 90 reached the stock support module's `annotationlib` import and 40 reached the unsupported `unittest` import used by the original mocks. The first diagnostic for each other method is in its record. Separate host-driven and mixed results retain their actual execution roles.

Reproduce the method attempts with the pinned interpreter and [vendored shared fixtures](../shared/UPSTREAM.json):

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/full/shutil/attempt_methods.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --compiler-tree c5bbd358d5a85df154c128131af824551bfa805a --result /tmp/method-attempts-linux.json
```

Pass `--method-attempts /tmp/method-attempts-linux.json` to the evidence runner to use this result.
