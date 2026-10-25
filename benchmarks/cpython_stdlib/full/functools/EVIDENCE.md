# Native functools evidence

Each observation uses the [shared per-occurrence contract](../../SUITE_RUNNER.md)
and includes every upstream ID at both optimization levels.

## Latest complete Linux measurement

[Compiler and binary hashes](observations/8656b43/source.json) identify the exact
source used for both optimization levels. The unchanged `TestSingleDispatch.test_simple_overloads` now registers and calls native implementations for a string, integer, and list and makes its three assertions in Imre. CPython runs the module, class,
and instance hooks; the compiled child creates its own native test instance.
Compare the [parent measurement](observations/308a1e8/evidence-linux.json) to check which individual occurrences changed.

| Unchanged suite | Occurrences | Fully native | Native body, host fixture | Host-driven native operation | Mixed | Failed | Blocked |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| [Primary](observations/8656b43/evidence-linux.json) | 325 | 0 | 9 | 17 | 9 | 0 | 290 |
| [Free-threading](observations/8656b43/evidence-free-threading-linux.json) | 4 | 0 | 0 | 0 | 0 | 4 |

## Canonical main at `56731b6`

The [Linux job](https://github.com/astral-sh/imre/actions/runs/40179400348/job/121127002553)
checked out [the #766 squash](https://github.com/astral-sh/imre/commit/56731b668c9d44bfbcbcbec42f8a4e12b25e448c),
verified tree `5a4cdf11e5ac7cd54628b5c7705913392e6a89dc`, then built that compiler
and runtime. It reproduced both pinned CPython references. Counts were identical
at `-O0` and `-O3`:

| Original Linux suite | Reference passes | Native bodies with host fixtures | Host-driven native operations | Mixed | Failed | Blocked |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| [test.test_functools](observations/56731b6/evidence-linux.json) | 325 | 2 | 15 | 13 | 0 | 295 |
| [test.test_free_threading.test_functools](observations/56731b6/evidence-free-threading-linux.json) | 4 | 0 | 0 | 0 | 0 | 4 |

The two C and Python `test_with_bound_and_unbound_methods` bodies constructed
and called `partial` and made their assertions in Imre; CPython ran the fixtures.
Of the other 323 primary bodies, 174 stopped during fixture compilation, 128
during method compilation, and 21 at a method decorator. The separate cache
and reduce probes drove some of those operations from the host. On the
free-threaded CPython 3.14.5 build, all four original threaded tests passed with
the GIL disabled. The four native attempts stopped at the original `random`
import before any worker ran. Neither suite has a fully native fixture, a
reference-only occurrence, a skip, or an exclusion in this run.

## Single dispatch development branch at `392c8a5`

The [Linux suite run](https://github.com/astral-sh/imre/actions/runs/40183958493/job/121140692421)
built [#919's compiler](https://github.com/astral-sh/imre/commit/392c8a54e4b1626a890557dfd67e0fb8241986a8),
tree `1409babace258d586ed6cb1cadb2d3b2e0c42249`, and reproduced both Linux
references byte for byte. Both optimization levels produced the same counts:

| Original Linux suite | Reference passes | Native bodies with host fixtures | Host-driven native operations | Mixed | Failed | Blocked |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| [test.test_functools](observations/392c8a5/evidence-linux.json) | 325 | 5 | 15 | 13 | 0 | 292 |
| [test.test_free_threading.test_functools](observations/392c8a5/evidence-free-threading-linux.json) | 4 | 0 | 0 | 0 | 0 | 4 |

`TestSingleDispatch.test_simple_overloads` registers an integer implementation
and calls the dispatcher with a string, an integer, and a list. Imre executed
the entire unchanged method and all three string assertions. CPython ran the
unittest hooks. The other four native bodies are the C and Python variants
of `test_get_annotations` and `test_with_bound_and_unbound_methods`.

Of 320 bodies that did not compile, 174 stopped in fixture code, 125 in the
method, and 21 at a method decorator. For example, `test_register_decorator`
reads its unannotated integer argument; `test_wrapping_attributes` reaches a
missing function attribute; and `test_mro` stops on a local class declaration.
The four companion cases still stop on `import random` before any worker starts.
The older cache branch below and this dispatch branch contain different changes;
their counts describe separate compilers.

## Development branch at `4735cac`

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

## Development branches: the live `builtins.len` binding

The two Linux builds reproduced the pinned CPython 3.14.5 references byte for byte. Each row applies to both `-O0` and `-O3`.

| Tested source | Original suite | Reference passes | Native bodies with host fixtures | Host-driven native operations | Mixed | Failed | Blocked |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| [`8e8ce2b`](https://github.com/astral-sh/imre/actions/runs/40188541388/job/121154397915) | [test.test_functools](observations/8e8ce2b/evidence-linux.json) | 325 | 2 | 13 | 15 | 0 | 295 |
| [`8e8ce2b`](https://github.com/astral-sh/imre/actions/runs/40188541388/job/121154397915) | [test.test_free_threading.test_functools](observations/8e8ce2b/evidence-free-threading-linux.json) | 4 | 0 | 0 | 0 | 0 | 4 |
| [`af565ad`](https://github.com/astral-sh/imre/actions/runs/40189824549/job/121158245589) | [test.test_functools](observations/af565ad/evidence-linux.json) | 325 | 4 | 13 | 13 | 0 | 295 |
| [`af565ad`](https://github.com/astral-sh/imre/actions/runs/40189824549/job/121158245589) | [test.test_free_threading.test_functools](observations/af565ad/evidence-free-threading-linux.json) | 4 | 0 | 0 | 0 | 0 | 4 |

On the [fixture attribution branch](https://github.com/astral-sh/imre/pull/939), both `test_lru_reentrancy_with_len` occurrences replaced the real CPython builtin, made 13 lookups in the native cache, and invoked CPython’s original `len` on 10 misses. The supplemental cache snapshot was `(3, 10, 4, 4)` and the original binding was restored by identity. The host ran the method and its assertions.

On the [native binding branch](https://github.com/astral-sh/imre/pull/949), the same two unchanged methods saved, replaced, called and restored Imre’s live `builtins.len`. The method bodies, assertions, cache and length operations all ran in Imre; the original unittest hooks ran on CPython. Neither source ran either suite with native fixtures, and both companion runs stopped at `random` before any native worker started. These source-specific observations do not replace the canonical reports.

## Dispatch arguments and the #726 annotation integration

Each build reproduced both pinned CPython references byte for byte. Counts apply independently at `-O0` and `-O3`.

| Compiler and report | Native bodies with host fixtures | Host-driven native operations | Mixed | Blocked |
| --- | ---: | ---: | ---: | ---: |
| [270129b](https://github.com/astral-sh/imre/actions/runs/40193095109/job/121168103466) [primary, 325](observations/270129b/evidence-linux.json) | 5 | 15 | 13 | 292 |
| [270129b](https://github.com/astral-sh/imre/actions/runs/40193095109/job/121168103466) [free-threading, 4](observations/270129b/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| [581e518](https://github.com/astral-sh/imre/actions/runs/40193838962/job/121170327420) [primary, 325](observations/581e518/evidence-linux.json) | 4 | 13 | 15 | 293 |
| [581e518](https://github.com/astral-sh/imre/actions/runs/40193838962/job/121170327420) [free-threading, 4](observations/581e518/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |

On `270129b`, the unchanged registration method compiles its formatting and stops at `self.assertIs` on upstream line 2297. `test_simple_overloads` runs its native body and assertions under host fixtures. On `581e518`, the four native bodies are the C and Python variants of `test_argument_checking` and `test_with_bound_and_unbound_methods`. Both LRU reentrancy methods remain mixed on that source. Neither build starts a free-threaded native worker.

## Local full-suite reruns

Each source ran with the original CPython 3.14.5 fixtures. The reports list every occurrence at O0 and O3. Counts match between levels.

| Source and suite | Native bodies with host fixtures | Host-driven native operations | Mixed | Blocked |
| --- | ---: | ---: | ---: | ---: |
| [b54496f](https://github.com/astral-sh/imre/commit/b54496f44e7e38498b6202afa518b2cd57e27dfe) [primary, 325](observations/b54496f/evidence-linux.json) | 4 | 13 | 15 | 293 |
| [b54496f](https://github.com/astral-sh/imre/commit/b54496f44e7e38498b6202afa518b2cd57e27dfe) [free-threading, 4](observations/b54496f/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| [bc9c98c](https://github.com/astral-sh/imre/commit/bc9c98c23318e56493bfb6ca8e08e33847787c05) [primary, 325](observations/bc9c98c/evidence-linux.json) | 5 | 15 | 13 | 292 |
| [bc9c98c](https://github.com/astral-sh/imre/commit/bc9c98c23318e56493bfb6ca8e08e33847787c05) [free-threading, 4](observations/bc9c98c/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| [ecf60bc](https://github.com/astral-sh/imre/commit/ecf60bcde79ee43463177a53fbe9ddf0b865fe81) [primary, 325](observations/ecf60bc/evidence-linux.json) | 6 | 15 | 13 | 291 |
| [ecf60bc](https://github.com/astral-sh/imre/commit/ecf60bcde79ee43463177a53fbe9ddf0b865fe81) [free-threading, 4](observations/ecf60bc/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |

On `b54496f`, keyword-rest parsing and deferred annotation names coexist after the canonical I/O repairs. On `bc9c98c`, `TestSingleDispatch.test_wrapping_attributes` stops at `sys.flags.optimize`. On `ecf60bc`, the unchanged body and both assertions run in Imre under the original host fixtures using the actual optimization level, wrapper name, and nullable docstring. The four companion cases on each source stop before native worker execution.
