# A child disappears before Popen.send_signal

The original CPython 3.14.5 `POSIXProcessTestCase.test_send_signal_race2` has
no explicit assertion. It reaps a child that exits 1, installs a replacement
`poll` on that instance, clears `returncode`, sends SIGTERM, restores the
inherited method and calls `kill()`. Its successful return alone cannot
distinguish a correct signal attempt from a no-op.

[Paired receipts](instance-poll-linux.json) on Imre `838784d` (#1075) record,
at O0 and O3, one native parent launching the pinned interpreter for the
original `-c 'exit(1)'` command. The original CPython mock installs its
unchanged `lambda: None` on that native Popen and removes it at scope exit.
The parent's `wait4` reaps exit 1, then its one `kill(pid, SIGTERM)` gets
ESRCH. After restoration a new `wait4` on that PID gets ECHILD; there is no
SIGKILL. Keeping the patch only on the host proxy lets the same method
finish without any native kill syscall. That control is not counted as a pass.

The operation runs in Imre. The body, its implicit no-exception outcome and
the original mock context run on CPython. Compiling the whole method reaches
`with mock.patch.object(p, 'poll', new=lambda: None)` and stops at the
unavailable shared patch API. The child runs the original CPython data
program. One occurrence changes per level; the other 352 stay untouched.

Run `instance_poll_originals.py` using pinned CPython on Linux with strace,
passing `--imre`, `--runtime`, `--clang`, `--opt-level 0` or `3`, and
`--result`. To apply an inspected pair to a later module report:

```sh
python3 instance_poll_evidence.py --previous evidence-linux.json \
  --receipt instance-poll-linux.json --result /tmp/subprocess-evidence.json
python3 ../../suite_evidence.py reference-linux.json /tmp/subprocess-evidence.json
```
