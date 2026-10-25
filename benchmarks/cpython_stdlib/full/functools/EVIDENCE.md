# Native functools evidence

Both reports use the [shared per-occurrence contract](../../SUITE_RUNNER.md).
Each has a row for every upstream ID at each optimization level.

| Pinned Linux suite | Reference | Native bodies with host fixtures | Host-driven native operations | Mixed | Failed | Blocked |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| [test.test_functools](evidence-linux.json) | 325 | 2 | 13 | 12 | 1 | 297 |
| [test.test_free_threading.test_functools](evidence-free-threading-linux.json) | 4 | 0 | 0 | 0 | 0 | 4 |

Counts are identical at -O0 and -O3. There are no fully native fixtures or
reference-only rows in this observation. The ordinary failure is
TestReducePy.test_reduce_with_kwargs (the pure variant's deprecated keyword
binding). The two complete native bodies are the C and Python variants of
test_with_bound_and_unbound_methods. Imre constructs and calls both partials,
performs both string comparisons and runs the test body. CPython supplies the
original module/class/instance hooks; the child constructs its native instance.

**Primary evidence.** run_bodies.py verifies the pinned file, then attempts
all 325 complete methods at both levels with the shared runner. It keeps
fixture compilation, method compilation and unsupported method decorators
distinct. write_evidence.py joins those attempts to the original reduce and
cache method executions. Its product_probe observations include native call
counts, host observations and exact failures. A method blocked during native
compilation can still have a separate observed host-driven result. See the
existing reduce and cache gates for the ABI and supported argument layouts.

The complete-body observation used aef6c3eb28515f175abdd7433800774e848d2d14,
the integration of the two public heads 7ebe2ff52c4d45bf5ac2b1a39143baf9709b2cac
([partial](https://github.com/astral-sh/imre/pull/559)) and
0bb3ad5decf070953c4fb5114605aae6582ab68d
([static bindings](https://github.com/astral-sh/imre/pull/567)).
The [cache gate](https://github.com/astral-sh/imre/pull/535) was rerun at
a8653f136ff14f3e6bc2b26e1e3f60179c329058. [Reduce](https://github.com/astral-sh/imre/pull/514)
was measured at bced8ce69cfe682d5d6b9c237b1282ef1de0dfa7. Each row records
the compiler used by its respective observation.

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

To reproduce, run from this directory and use a test-enabled CPython 3.14.5
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

The reduce runner returns nonzero while its ordinary failure remains; it
still writes the individual results. The companion gate rejects a GIL-enabled
interpreter instead of treating the package-level skip as four executions.
