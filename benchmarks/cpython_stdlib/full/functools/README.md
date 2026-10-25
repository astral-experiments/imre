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

Both optimization levels have the same results. [The reduce report](reduce-linux.json)
records all seven IDs, their native call counts and separate keyword controls.

All seven complete methods were also submitted to the shared native-method
runner under their original CPython fixtures. The test bodies stop at local
class declarations, unannotated local functions or `assertWarns`. Their first
diagnostics are recorded separately from the native folds.

| Execution at each of -O0 and -O3 | IDs |
| --- | ---: |
| Complete native test body | 0 |
| Host assertions and callbacks; Imre drives all reduce operations | 4 |
| Mixed: native folds plus one missing-arguments error from host binding | 2 |
| Mixed: two native folds with host keyword binding and deprecation warnings | 1 |
| Other primary IDs outside this reduce run | 318 |
| Total | 325 |

`run_reduce.py` replaces only the two test classes' interface to reduce.
The compiled driver calls public `functools.reduce` with no seed, a positional
seed, or `initial=`. The host retains the original objects, supplies their
iterator protocol, and executes the provided Python callbacks. Each pull and
callback invocation in the seven scored methods is requested by Imre. The
pure variant accepts `function=` and `sequence=`, warning once per call before
binding. The accelerator rejects these forms; both accept `initial=` without
a warning. The driver compares the result or exception class and the warning
category and text against both original variants in separate, unscored controls.

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
The historical driver behind the table supplied named-field reads on the host.
The current driver keeps each immutable `CacheInfo` in the child and sends the
original snapshot handle back for named reads. It also invokes the native
`_CacheInfo` constructor for the expected tuples; CPython still compares the
transported values. A separate transport check reads four snapshots out of order
after a clear. Its reads do not contribute to any upstream occurrence.

```sh
python3.14 benchmarks/cpython_stdlib/full/functools/run_cache.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /usr/bin/clang \
    --result /tmp/functools-caches.json
```

### Independently created wrappers

`cache(f)`, `lru_cache(f)` and `lru_cache(maxsize, typed=...)(f)` allocate a
native wrapper for synchronous functions with exact positional keys. Its frame
retains the actual callable and its own cache. A second application starts
empty even at the same source location. Aliases share entries and counters;
`__wrapped__` calls bypass the cache. Module decorators use the same cache
state and eviction operations.

`tests/fixtures/callables/owned_caches.py` compares two independent wrappers,
nested wrappers, disabled and unbounded caches, exception misses and clearing
against the reference interpreter. These are native product checks; they do
not add any unittest occurrences to the table above.

### Cache parameters

[`cache-parameters-linux.json`](cache-parameters-linux.json) records a later
paired run of all 64 cache methods on compiler `ebc346a2`. The original
`TestLRUC.test_lru_cache_parameters` and `TestLRUPy.test_lru_cache_parameters`
each read two fresh native dictionaries at O0 and O3. The fixture transports
their complete representations back to CPython for the original assertions.
The complete native bodies still stop at the second local definition of `f`.
The other 62 cache methods returned the same results and operation counts as
on `133d1e83`. At each level, 11 of 64 used host-driven native operations,
10 were mixed, and 43 were blocked. The full native-body run on this branch
completed two of 325 methods under host fixtures; all four free-threaded
methods remained blocked.

The native callable fixture also checks mutation, key order, nested caches,
and calls through a `Callable` parameter or return. For example, decorating a
cache configured with `maxsize=2, typed=True` with `lru_cache(3)` preserves
the inner public parameters, but the new wrapper's `cache_info()[2]` is 3.

### Local cache decorators

At `5144a54` each executed local `@cache` or `@lru_cache` definition
allocates a wrapper and table. Redefining the name leaves earlier aliases
and their entries intact. [The native fixture](../../../../tests/fixtures/callables/local_caches.py)
checks hits, clearing, callback use, annotations, and a class-held module
alias against CPython at O0 with ASan and O3 (365 output bytes at each level).

Both unchanged `test_lru_cache_parameters` methods get past declaration
collection and reach the first assertion on upstream line 2120. Its native
`unittest.TestCase.assertEqual` fixture expects integers and rejects the
dictionary, so neither complete body is a native pass on that compiler.
