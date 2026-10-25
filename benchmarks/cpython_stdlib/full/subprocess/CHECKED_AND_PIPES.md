## Original methods and native operations

The [full method attempts](method-attempts-linux.json) record every one of the
353 Linux occurrences at both optimization levels. The compiler/runtime was
[`4851037`](https://github.com/astral-sh/imre/commit/4851037788ee9b71813db9d507bf84ff444ce61a),
tree `d5ada98d3fab09be28ec2af344338dd4c3c9e9f2`. The original CPython fixture
runner supplies the module, class and instance hooks; the compiled body uses a
native case instance. No upstream method, expected result or skip was changed.

Each all-method sweep completed 42 bodies, stopped compilation for 272, and
observed 39 reference skips. The sweep explicitly enables the two CPU-gated
failure-cleanup cases; the default reference and canonical reports retain
all **41** original skips. The six checked-output methods in each poll variant
use typed membership assertions. They completed separately and do not overlap
the 42. A completed body is an execution receipt; it earns a native-operation
classification only where the operation itself has been checked. The fixture
remains CPython, and zero occurrences are fully native.
The earlier typed subclasses of `unittest.TestCase` no longer compiled once
`assertIn` gained a dictionary parameter. The local case now uses native
`bytes in bytes` or `str in str` and calls the bundled `TestCase.assertTrue`
or `assertFalse`. Each case runs the original assertion call and body. See
the [unittest handoff](https://github.com/astral-sh/imre/pull/508#issuecomment-6397252339).

[Selected operations](selected-operations-linux.json) records the evidence
that changes 25 rows. Nine complete methods check validation, checked exits
and deadlines. The native parent reaped status 47 and 5 from the requested
Python commands, and killed and reaped the timeout child with SIGKILL after
the selected deadline. The NUL command and NUL executable were rejected
before either could fork. The successful checked run launched the original
module fixture's `/usr/bin/true` and returned status zero. Twelve complete
checked-output methods verified child stdin, merged output and text/bytes.
For the low error-pipe method, the parent used fd 0 and 1 for the CLOEXEC
launch socket, waited for its Python child, then restored both fd numbers.

Three other unchanged methods retain **host bodies and assertions**. For
`test_communicate_eintr` (both selector fixtures), the original children sent
SIGUSR1 to their native parent. Its stdout and stderr reads were interrupted
and retried on the same fd; both reached EOF and both children were reaped.
The handler in the native parent forwarded each delivery over the fixture socket.
The original Python signal callback ran once for each data child before the
corresponding `communicate()` returned.
The one-pipe path made no selector calls in either EINTR method. Both are
host-driven native subprocess operations.

`test_select_unbuffered` is **mixed**. Imre read four bytes; the host received
the same open file description via SCM_RIGHTS and called its real `select()`.
The returned readable object was the exact stream passed by the original
method, with one byte left in the kernel. Imre closed the stream and reaped the
child. The small [C fixture](fixture_pipes.c) transfers signal disposition or an
existing fd.

The [integrated readiness run](pipe-readiness-integrated-linux.json) repeats
the unchanged method through two observers. Its hooks, assertions, and cleanup
ran in CPython. The native parent read `appl`, left one byte queued, closed
stdout, and reaped the original Python data child once. Both the procfs and
SCM_RIGHTS observers saw host `select()` return the original stream. With the
writer held open, the raw pipe remained ready; the buffered pipe had read all
five bytes and was not ready.

The [operand control](readiness_operands.py) distinguishes the public calls:
`select([f.fileno()], [], [], 0.0)` makes one native kernel readiness call;
`select([f], [], [], 0.0)`, the call in the original test, rejects `BinaryIO`
where Imre expects an integer. The occurrence remains mixed at O0 and O3.
The complete native method attempt still stops at `import test`.

### Reproduction on Linux

Build the named source revision and use the pinned 3.14.5 interpreter,
`strace`, and Clang. Each driver accepts the same tool flags. Run each at both
levels (`$OPT` = 0, 3; `$OUT` is a separate results directory):

```sh
cd benchmarks/cpython_stdlib/full/subprocess
"$CPYTHON" attempt_bodies.py --trace-nopoll --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" --opt-level "$OPT" --result "$OUT/attempts-$OPT.json"
for driver in checked_error_originals native_checked_output low_errpipe pipe_fixture_originals; do
  "$CPYTHON" "$driver.py" --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" --opt-level "$OPT" --result "$OUT/$driver-$OPT.json"
done
"$CPYTHON" selected_operation_evidence.py --raw-directory "$OUT" --imre-revision "$REV" --tested-tree "$TREE" --previous evidence-linux.json --result "$OUT/evidence-linux.json"
"$CPYTHON" ../../suite_evidence.py reference-linux.json "$OUT/evidence-linux.json"
```

The generator applies only the measured operations. Other observations,
failures, reference outcomes and their fixture roles are preserved.

To reproduce the overlap independently, run `readiness_original.py`,
`pipe_fixture_originals.py --test-id test.test_subprocess.POSIXProcessTestCase.test_select_unbuffered`,
and `readiness_operands.py` with the same compiler, runtime, Clang and level
arguments shown above. By default `pipe_fixture_originals.py` also exercises
both selector variants of `test_communicate_eintr`.
