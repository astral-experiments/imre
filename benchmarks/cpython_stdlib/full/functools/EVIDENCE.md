# Native functools evidence

Both reports use the [shared per-occurrence contract](../../SUITE_RUNNER.md).
Each has a row for every upstream ID at each optimization level.

The [tested compiler and drivers](https://github.com/astral-sh/imre/commit/4735cacfa95c66d17b8942d4882b291f7088861d)
come from [the cache parameter comparison PR](https://github.com/astral-sh/imre/pull/811).
The reports describe that compiler revision, which includes the local cache
decorator and cache annotation changes. They do not describe a build of main.
The same compiler ran the complete-method, cache, reduce and free-threaded checks
at both levels, including the [corrected reduce fixture](https://github.com/astral-sh/imre/pull/713).
Of the 325 primary methods, 174 stopped while compiling fixture code, 124 while
compiling the method, and 21 at an upstream method decorator. Six bodies and
their assertions completed in Imre with the original host fixtures.

| Pinned Linux suite | Reference | Native bodies with host fixtures | Host-driven native operations | Mixed | Failed | Blocked |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| [test.test_functools](evidence-linux.json) | 325 | 6 | 13 | 13 | 0 | 293 |
| [test.test_free_threading.test_functools](evidence-free-threading-linux.json) | 4 | 0 | 0 | 0 | 0 | 4 |

Counts are identical at -O0 and -O3. There are no fully native fixtures or
reference-only rows in this observation. TestReducePy.test_reduce_with_kwargs
now runs two native folds with two host bindings and two host warnings. The
earlier positional-only fixture rejected its first call before reaching Imre.
The C and Python variants of test_with_bound_and_unbound_methods
construct and call both partials and compare the resulting strings in Imre.
The C and Python variants of test_lru_cache_parameters each decorate two local
functions, call cache_parameters on each wrapper, and compare the returned
dictionaries in Imre. The original module and unittest fixture run in CPython.
The C and Python variants of test_get_annotations construct the cache wrapper,
read the original and wrapper annotations, and compare both dictionaries in
Imre. CPython supplies the original module/class/instance hooks; the child
constructs its native instance. The two deferred-reference methods stop while
compiling the original test.support.EqualToForwardRef.__eq__.

The [previous published report](https://github.com/astral-sh/imre/pull/716)
observed four native bodies. On the [intermediate compiler](https://github.com/astral-sh/imre/commit/2a28a2259587692d74d12ddb2974e9af32433673),
CPython ran the two parameter test bodies and drove two native dictionary
reads per body. Comparing the current run with that intermediate run, only
those two primary classifications changed. The 64 cache and seven reduce
operation probe results, including the host observations, are byte-identical.

**Primary evidence.** run_bodies.py verifies the pinned file, then attempts
all 325 complete methods at both levels with the shared runner. It keeps
fixture compilation, method compilation and unsupported method decorators
distinct. write_evidence.py joins those attempts to the original reduce and
cache method executions. Its product_probe observations include native call
counts, host observations and exact failures. A method blocked during native
compilation can still have a separate observed host-driven result. See the
existing reduce and cache gates for the ABI and supported argument layouts.

**Cache regression.** An [earlier build](https://github.com/astral-sh/imre/commit/08761095d9a555174f2f3a77e29a3503aca31334)
failed seven original reentrant cache cases at each level: a native C extension
returned an integer, but its stub's ellipsis had been compiled as a Python
body returning None. Fixture cleanup initially obscured the native error.
[Before/after observations](cache-reentry-linux.json) retain all 14 failure
rows, the child diagnostic for an original test, and the working control.
With the stub fix, the complete 64-occurrence [cache report](cache-linux.json)
matches the previous successful run byte for byte at both levels. The
[preceding complete-method observation](https://github.com/astral-sh/imre/commit/2f137d67646331a4445f22f63f8895502a0c0a16)
had the two partial bodies and no complete cache annotation body.

**Companion evidence.** On a normal 3.14.5 Linux build the original
test.test_free_threading package raises unittest.SkipTest("GIL enabled")
at __init__.py:8, before discovering any methods. A second build of the
same pinned source with --disable-gil --enable-test-modules had
Py_GIL_DISABLED=1 and sys._is_gil_enabled() == False. All four original
methods passed with the original ten-thread Barrier fixture on that build.
Their [reference](reference-free-threading-linux.json) is independent of the
325 primary cases. run_free_threading.py submits each whole method and its
unchanged helper to Imre. Both helper bodies use random.randint, Barrier,
Thread and threading_helper.start_threads. Compilation first stops at the
original random import. Zero concurrent operations ran in Imre.

To reproduce, use the linked compiler and body/cache drivers plus run_reduce.py
from the corrected fixture PR. Run from this directory and use a test-enabled CPython 3.14.5
for PY and a free-threaded build of the same commit for PYT. TOOL_ARGS stands
for --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang.

```sh
$PY run_bodies.py $TOOL_ARGS --compiler-commit "$BODY_COMMIT" --result /tmp/bodies.json
$PY run_reduce.py $TOOL_ARGS --result /tmp/reduce.json
$PY run_cache.py $TOOL_ARGS --result /tmp/cache.json
$PY write_evidence.py --bodies /tmp/bodies.json --reduce /tmp/reduce.json --cache /tmp/cache.json \
    --reduce-commit "$REDUCE_COMMIT" --cache-commit "$CACHE_COMMIT" --result evidence-linux.json
$PYT ../../suite_inventory.py FREE_THREADING.json --result reference-free-threading-linux.json
$PYT run_free_threading.py $TOOL_ARGS --compiler-commit "$BODY_COMMIT" --result evidence-free-threading-linux.json
$PY ../../suite_evidence.py reference-linux.json evidence-linux.json
$PYT ../../suite_evidence.py reference-free-threading-linux.json evidence-free-threading-linux.json
```

The reduce runner writes each individual result and returns nonzero on a
test failure. The companion gate rejects a GIL-enabled
interpreter instead of treating the package-level skip as four executions.
