## A mocked stopped status and a real kernel stop

The unchanged Linux `test_stopped` first reaps the actual child through
`support.wait_process`. It then patches `subprocess.os.waitpid` to return
`(proc.pid, _testcapi.W_STOPCODE(3))`. The original assertion expects
`proc.wait() == -3`.

The host still runs that body, its mock, assertions and Python fixture hooks.
The native fixture performs the first real wait in the child's kernel parent.
Inside the mock's scope, the native Popen makes its own libc waitpid call. A
fixture linked **only into that test executable** forwards this request to
the original mock. It records the PID and flags, receives the raw tuple and
returns it to the native caller. The mock was called once with the right PID
and WNOHANG. Imre decoded the returned `0x37f` and cached `-3`. The separate
original signal-race method still receives ECHILD after an external reap and
sends no signal; its live-child control sends SIGTERM and reaps `-15`.

The [paired report](stopped-status-linux.json) distinguishes that fixture from
a supplemental **real Linux ptrace stop**. A C data child requests tracing and
raises SIGQUIT. A compiled Imre parent observes the kernel status in either
`Popen.poll` or `wait`, then repeats the cached wait. A fixture resumes the
child and independently reaps its final exit 7. Before the fix, the native
results in both modes were `[0, 0, 7]`. After [#815](https://github.com/astral-sh/imre/pull/815),
they match pinned CPython's `[-3, -3, 7]` at both optimization levels.
The trace follows the parent only so that the child's PTRACE_TRACEME works.

Run each command at `$OPT=0` and `3` on Linux with ptrace allowed:

```sh
"$CPYTHON" signal_race_originals.py --transfer-stopped-mock --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" --opt-level "$OPT" --result "$OUT/original-$OPT.json"
"$CPYTHON" stopped_status.py --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" --opt-level "$OPT" --result "$OUT/kernel-$OPT.json"
"$CPYTHON" stopped_evidence.py --previous evidence-linux.json --result "$OUT/evidence-linux.json"
```

The unarmed fixture still calls the kernel directly. The original body attempt,
including its decorator diagnostic, is preserved. Only one of the 353
occurrences is updated: the kernel controls add no test IDs.
