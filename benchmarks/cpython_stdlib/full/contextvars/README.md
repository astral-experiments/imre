# Context variables

This fixture vendors `Lib/contextvars.py`, `Lib/test/test_context.py`, and
the PSF license verbatim from CPython 3.14.5. `UPSTREAM.json` pins their Git
blobs and upstream commit. The oracle has 56 original test cases: 33 public
ContextTest methods and 23 methods for CPython's private HAMT implementation.
Default Linux regrtest passes 55 and skips the CPU resource stress test.

At each optimization level, two original methods execute entirely in Imre:
`test_context_run_1` and `test_context_typerrors_1`, including their actual
native unittest regex assertions. Another 12 original methods execute their
context operations in Imre. For those 12, the methods, decorators, callbacks, and assertions run in
CPython. A fresh native process per method owns the contexts, variables, and
tokens. Its public `Context.run` and token `with` blocks actually enter and
leave context while the original callback executes. When that callback raises a
`ValueError`, the fixture raises it inside the native callback as well.
`native-linux.json` identifies the execution role, byte digest, and operation
counts for each method, and accounts for all 56: two fully native, 12 with
host-driven native operations, 41 reference-only, and one upstream resource skip.

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
