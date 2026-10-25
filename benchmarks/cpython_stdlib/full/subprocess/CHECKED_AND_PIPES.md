## Original methods and native operations

The [full method attempts](method-attempts-linux.json) record every one of the
353 Linux occurrences at both optimization levels. The compiler/runtime was
[`12a645f`](https://github.com/astral-sh/imre/commit/12a645f666187b35ebfaa4c53ad152187bb356d7),
tree `3d7be7793cd60f833814e5fb0ebdb7e23d2f4c26`. The original CPython fixture
runner supplies the module, class and instance hooks; the compiled body uses a
native case instance. No upstream method, expected result or skip was changed.

Each all-method sweep completed 38 bodies, stopped compilation for 276, and
observed 39 reference skips. The sweep explicitly enables the two CPU-gated
failure-cleanup cases; the default reference and canonical reports retain
all **41** original skips. The six checked-output methods in each poll variant
need typed membership assertions. They completed separately and do not overlap
the 38: **50 native bodies with host fixtures**, zero fully native occurrences.
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
The one-pipe path made no selector calls. For `test_select_unbuffered`, the
host's select received the same open file description via SCM_RIGHTS. The
native read took four bytes; FIONREAD found one byte left in the kernel. The
small [C fixture](fixture_pipes.c) transfers signal disposition or an existing
fd; the compiled process owns launch, communication, file operations and wait.

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
