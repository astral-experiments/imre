# Context variables

This fixture vendors `Lib/contextvars.py`, `Lib/test/test_context.py`, and
the PSF license verbatim from CPython 3.14.5. `UPSTREAM.json` pins their Git
blobs and upstream commit. The oracle has 56 original test cases: 33 public
ContextTest methods and 23 methods for CPython's private HAMT implementation.
Default Linux regrtest passes 55 and skips the CPU resource stress test.

## Mapping and ContextVar name hashing

The unchanged CPython 3.14.5 suite passed 56 of 56 methods on Linux. Imre
passed 31 at O0 and O3, using the source changes merged in [#8184](https://github.com/astral-experiments/imre/pull/8184)
and [#8190](https://github.com/astral-experiments/imre/pull/8190). Each
saved executable reproduced its FILE run after the first-party source and
build cache were withdrawn. The [per-method table](paired-8190-source.tsv)
includes both runs and the digest of every original method.

Registering `Context` as a `Mapping` fixed `test_context_isinstance`.
Hashing a `ContextVar` name at construction fixed
`test_context_new_unhashable_str_subclass`. The two thread methods errored
when the callbacks needed the shared Python heap. The 23 HAMT methods
skipped because Imre lacks CPython's private `_testinternalcapi.hamt()`
test interface.

## The unmodified test file

On Linux, CPython 3.14.5 ran all 56 methods when invoked directly with
`-B Lib/test/test_context.py -v ContextTest HamtTest`. The ordinary Imre
FILE command on [#8074](https://github.com/astral-experiments/imre/pull/8074)
collected the same 56 at both optimization levels.

| Runtime | Passed | Assertion failures | Errors | Skipped |
| --- | ---: | ---: | ---: | ---: |
| CPython 3.14.5 | 56 | 0 | 0 | 0 |
| Imre O0 | 29 | 2 | 2 | 23 |
| Imre O3 | 29 | 2 | 2 | 23 |

The [#8032 run](ordinary-linux-8032.json) passed 25 methods.
`test_context_run_1` now passes because `Context.run()` reports the missing
positional argument the method expects. Its [callback binding change](https://github.com/astral-experiments/imre/pull/8045) leaves the other 55 outcomes unchanged.

The [#8050 run](ordinary-linux-8050.json) passed 26 methods.
In `test_context_getset_1`, the token now exposes [`Token.MISSING`](https://github.com/astral-experiments/imre/pull/8062). The other 55 outcomes matched #8050.

The [#8066 run](ordinary-linux-8066.json) passed 27 methods. The two constructor tests now pass because `Context` and `ContextVar` report argument errors in the same order as CPython. The other 54 outcomes matched.

All 33 public `ContextTest` methods ran. The upstream class guard skipped
the 23 private `HamtTest` methods because Imre lacks
`_testinternalcapi.hamt()`. Module execution with `-m test.test_context`
and both saved executables after source withdrawal reproduced every FILE
outcome. The [per-method ledger](evidence-linux.json) gives the result and
source digest for each case. The [launch record](ordinary-linux-8074.json)
identifies the shipping products, commands and streams.

The earlier [isolated harness](native-linux.json) ran two complete methods in
Imre. For a further 12, CPython ran the test and its assertions against native
context operations. The ordinary FILE command above includes the original
decorators, callbacks and assertions in the compiled application.

`focused.py` separately compares complete native programs to CPython in both
whole-module and reachable-only compilation at O0 and O3. They exercise
constructor and call-site defaults, mutation during argument evaluation,
same-name variables, shallow copies and retained object identity, missing and
previous values, failed and reordered token resets, exceptional context exit,
and nested token scopes. The asyncio cases suspend on a real subprocess: child
tasks capture on scheduling, bare await shares its caller's context, and a
cancelled child restores its token inside its own binding context.

## Supported scope

The public module exposes `ContextVar`, `Token`, `Context`, and `copy_context`.
Contexts support `copy`, length, truth, membership, indexing, `get`, and
`run` with a statically known positional signature. A copy shares a
reference-counted mapping root; the first write detaches that root and keeps
the same Python object values. This gives constant-time copies, with linear
work on the first write to a shared map. Private CPython HAMT performance and
iteration order are outside this implementation.

An annotated variable can retain a concrete scalar, container, or acyclic native
record layout. Unannotated variables without a default can store supported
boxed scalars and fixed tuples. In a token, `var` and comparisons of
`old_value is Token.MISSING` preserve identity for all admitted payloads; direct
reads of `old_value` currently require a boxable value layout. Constructors
that could retain the context through a token, callable, or recursively owned
object, and defaults requiring a user-defined repr, receive a compile diagnostic.
The interpreter's arbitrary hash/equality callbacks, Context mapping equality
or iteration, thread inheritance, explicit task `context=` arguments, and
keyword forwarding in `Context.run` remain unsupported.

Run the checks with a test-enabled CPython 3.14.5:

```sh
suite=benchmarks/cpython_stdlib/full/contextvars
python "$suite/focused.py" --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang
python "$suite/run.py" --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang --cpython-source /path/to/cpython --result /tmp/native-contextvars.json
cmp "$suite/native-linux.json" /tmp/native-contextvars.json
```

## Public #8127

The complete unchanged FILE and both saved executables reproduced the 29 passes, two failures, two errors and 23 private-HAMT skips at O0 and O3. Fresh CPython 3.14.5 passed all 56 original IDs. Each native ID retained its public #8074 outcome. The [paired rows](paired-public-8127.tsv) identify all original method source ranges and hashes.
