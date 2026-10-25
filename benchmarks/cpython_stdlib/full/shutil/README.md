# CPython 3.14.5: shutil

The pinned release contains one test module, `Lib/test/test_shutil.py`. Loading it with unittest discovers **223 distinct IDs**, including inherited methods on the bytes and zero-copy classes. The [manifest](UPSTREAM.json) points to the existing byte-for-byte copies of the test and CPython license. The [Linux reference](reference-linux.json) records every ID: 173 pass and 50 skip on this host. Each skip has the reason reported by CPython. These numbers describe the oracle only.

To reproduce the reference with a test-enabled build of the pinned release, use the shared inventory runner from [#479](https://github.com/astral-sh/imre/pull/479):

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/shutil/UPSTREAM.json --result /tmp/shutil-reference.json
```

The runner verifies both the vendored and installed test blobs, discovers the full suite, and runs regrtest in a temporary directory. It requires regrtest's IDs to agree with unittest discovery.
