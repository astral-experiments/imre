# CPython 3.14.5 functools

The pinned CPython tree contains one suite, `Lib/test/test_functools.py`, and
no `test_functools/` package. The test and PSF license are byte-for-byte
copies of the Git blobs recorded in `UPSTREAM.json`.

| Result on Linux | IDs |
| --- | ---: |
| Discovered (19 classes, including pure Python and accelerator variants) | 325 |
| Passed on the reference CPython build | 325 |
| Reference skips, errors or failures | 0 |

`reference-linux.json` records the outcome of every ID. These are reference
results only; the native denominator remains 325. Neither standalone API
examples nor successful CPython execution are native suite passes.

To reproduce using the shared inventory runner and a test-enabled CPython
3.14.5 build:

```sh
python3.14 benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/functools/UPSTREAM.json \
    --result /tmp/functools-reference.json
```

## Function caches

`run_cache.py` runs all cache and LRU classes under the original CPython test
fixtures with the shared suite runner. Each decorated function gets a native Imre
cache. The original callback runs in CPython on a miss and the native cache holds
its opaque result handle. Recursive callbacks reenter the same native process.
The driver sends the actual positional arguments: lookups, recency, eviction,
statistics and clearing run in Imre.

| Cache coverage at both `-O0` and `-O3` | IDs |
| --- | ---: |
| Entire body and assertions in Imre | 0 |
| Host body and assertions, native cache operations | 9 |
| Mixed (host named-field access, wrapper metadata or exception state) | 10 |
| Blocked, with a reason per ID | 45 |
| Other functools IDs outside this cache run | 261 |
| Full denominator | 325 |

`cache-linux.json` records every cache ID at each optimization level, including
partial operation counts for blocked methods. The native cache currently accepts
fixed positional key layouts; the transport supports exact integers and strings.
The observed snapshot is an immutable native tuple. Its named fields are supplied
by the host until Imre supports the public CacheInfo type.

```sh
python3.14 benchmarks/cpython_stdlib/full/functools/run_cache.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /usr/bin/clang \
    --result /tmp/functools-caches.json
```
