# Concurrent wait, external reap, and stopped-status mock

The [four original Linux occurrences](wait-races-linux.json) preserve the
unchanged CPython test bodies, assertions, and fixture hooks at O0 and O3.
The replay uses the verified Linux archive from the [prospective build](https://github.com/astral-sh/imre/actions/runs/40179519955)
for [the merged wait implementation](https://github.com/astral-sh/imre/pull/771).
The three passing operations and the stopped-status failure occupy their original
four slots in each 353-occurrence report.

| Original method | Native observation |
| --- | --- |
| `ProcessTestCase.test_threadsafe_wait` and `ProcessTestCaseNoPoll.test_threadsafe_wait` | The compiled owner's timed wait overlapped the original Timer's poll, kill and wait on a second native thread. That thread sent SIGKILL to the same original child, exactly one thread reaped it, and both callers repeatedly read −9. |
| `POSIXProcessTestCase.test_send_signal_race` | The original `support.wait_process` reaped the data child with raw waitpid in its actual kernel parent. Native `Popen.send_signal` subsequently observed ECHILD and sent no signal. A separate live-child control sent SIGTERM and returned −15. |
| `POSIXProcessTestCase.test_stopped` | **Failed** with `0 != -3`. The external reap reached the native parent. The original mock supplies a synthetic stopped status to the host's `subprocess.os.waitpid`; native `Popen.wait` instead observed the kernel's ECHILD and returned 0. |

The timer fixture captures the opaque handle actually borrowed by compiled
`Popen` and keeps its Python owner alive until its worker joins. The worker
uses the same poll, signal, and wait functions called in the emitted LLVM IR.
It does not implement a process operation or cache a return code. The host
checks the native parent's kernel thread group, the data program's exec, the
signal recipient, and the one successful reap. Neither poll fixture reaches a
pipe selector in these wait operations. The data child is the upstream CPython
sleep or exit command, not the native parent.

With the pinned interpreter, run `threadsafe_wait_originals.py` and
`signal_race_originals.py` with `--imre --runtime --clang --opt-level {0,3}
--result`. The native fixture extension uses Imre's header and static runtime;
these Linux observations also require strace. The separate native method
attempts record the missing native threading import, test support import, and
upstream method decorator. No whole-body pass is inferred from these results.

`wait_race_evidence.py --previous <previous-report> --wait-races
wait-races-linux.json --result evidence-linux.json` preserves every other
occurrence and every original reference skip.
