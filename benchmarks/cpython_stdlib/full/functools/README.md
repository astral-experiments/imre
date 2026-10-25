# CPython 3.14.5 functools

The primary suite is `Lib/test/test_functools.py`; there is no
`test_functools/` package. The separate free-threading package contains four
companion cases. [Per-occurrence evidence](EVIDENCE.md) records both suites
at both optimization levels. The tests and PSF license are byte-for-byte
copies of the Git blobs in `UPSTREAM.json` and `FREE_THREADING.json`.

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

## Native reduce

Both optimization levels have the same results. See `reduce-linux.json` for
every attempted ID, including the failure and native call counts.

All seven complete methods were also submitted to the shared native-method
runner under their original CPython fixtures. The compiler rejects the native
fixture's class assignment `reduce = staticmethod(functools.reduce)` before
checking the body. This setup blocker is recorded independently from the
outcome of running the original method against the native fold.

| Execution at each of -O0 and -O3 | IDs |
| --- | ---: |
| Complete native test body | 0 |
| Host assertions and callbacks; Imre drives all reduce operations | 4 |
| Mixed: native folds plus one missing-arguments error from host binding | 2 |
| Failed: pure Python reduce's deprecated keyword aliases and warning | 1 |
| Reference only, outside this reduce run | 318 |
| Total | 325 |

`run_reduce.py` replaces only the two test classes' interface to reduce.
The compiled driver calls public `functools.reduce` with no seed, a positional
seed, or `initial=`. The host retains the original objects, supplies their
iterator protocol, and executes the provided Python callbacks. Each pull and
callback invocation is requested by Imre. No host call to functools.reduce is
used. These results do not establish native support for arbitrary fixture
classes, their sequence protocols, or polymorphic callbacks.

```sh
python3.14 benchmarks/cpython_stdlib/full/functools/run_reduce.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
    --clang /path/to/clang --result /tmp/functools-reduce.json
```

The runner exits nonzero on any upstream error or failure. Compiling the
entire unchanged test module directly currently stops at line 2 on the
unsupported `annotationlib` import at both optimization levels.

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
