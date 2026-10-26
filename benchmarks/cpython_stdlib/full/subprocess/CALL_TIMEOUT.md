# Killing and reaping a child after subprocess.call times out

On Linux main `f4e461af62b885ebf1b62614334a1a2329bb251d`, the complete unchanged
`ProcessTestCase.test_call_timeout` and `ProcessTestCaseNoPoll.test_call_timeout` run
at O0 and O3. The native method calls `subprocess.call(..., timeout=0.1)`, and its
native `assertRaises` requires `TimeoutExpired`. CPython supplies the original
module/class/instance lifecycle; the requested child executable is the pinned
CPython interpreter running `while True: pass`. The Imre parent owns the launch,
wait, signal, reap and assertion. These are two native bodies with host fixtures.

[The receipts](call-timeout-linux.json) identify the compiler, runtime, native
binary and method by hash. For each occurrence, strace observes the child's
successful exec, the parent's nonblocking waits, a successful SIGKILL for that PID,
and one reap with SIGKILL status. As a control, the same native test binary runs
with `/bin/true` as its child: it observes exit 0, sends no signal, and the
unchanged assertion fails with `TimeoutExpired not raised by call`. Both fixture
variants exercise the timeout. This operation does not select or poll pipes.

To reproduce, run each level with the CPython 3.14.5 interpreter matching
[the manifest](UPSTREAM.json), a matching Imre executable and native runtime,
clang, and Linux strace:

```sh
"$PYTHON" -E -B call_timeout_originals.py --imre "$IMRE" --runtime "$RUNTIME" \
  --clang "$CLANG" --opt-level 0 --result /tmp/call-timeout-0.json
"$PYTHON" -E -B call_timeout_originals.py --imre "$IMRE" --runtime "$RUNTIME" \
  --clang "$CLANG" --opt-level 3 --result /tmp/call-timeout-3.json
python3 call_timeout_evidence.py --previous evidence-linux.json \
  --receipt call-timeout-linux.json --result /tmp/subprocess-evidence.json
python3 ../../suite_evidence.py reference-linux.json /tmp/subprocess-evidence.json
```

The paired evidence keeps all 353 occurrences and changes two at each level.

The [separate body sweep](body-attempts-f4e461a-linux.json) used
`attempt_bodies.py` with the same binaries and the CPU resource enabled. All
353 IDs were considered independently at each level: 40 complete methods
returned, 274 could not compile, and 39 received their upstream skip. The
two levels agreed per ID after replacing temporary directory names. The
default reference still has 41 skips. A completed body alone does not
establish where its operation ran.
