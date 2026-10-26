# Mock requirements for the unchanged subprocess fixtures

Subprocess mock requirements from an AST and source audit of CPython 3.14.5 at `5607950ef232dad16d75c0cf53101d9649d89115`: 13 definitions reference mock, reaching **17 of 353 IDs** after inherited methods and the three KeyboardInterrupt callers. The two Windows wait IDs are reference skips on Linux. The parent independently confirmed the entire module stops at line 2, `from unittest import mock`, on main `76d74f9`.

* Import and class construction need both `from unittest import mock` and `unittest.mock`. `patch("dotted.name")` and `patch.object(object, attr)` are decorators and context managers. Decorators inject the generated mock as a positional argument, including on a bound helper; an explicit `new=False` decorator injects nothing. Contexts return the replacement and restore the original on normal or exceptional exit; nested patches include a class, module, and individual instance. Explicit replacements also include a positional subclass and a keyword lambda.
* `ProcessTestCase.test_repr` (also NoPoll) patches `Popen._execute_child` to suppress launch. Windows-only `test_wait_negative_timeout` (also NoPoll) programs `_winapi.WaitForSingleObject` via `return_value` and asserts `assert_called_once_with(handle, 0)`.
* POSIX `test_exception_errpipe_{normal,bad_data}`: decorator patches `subprocess._fork_exec`; its assignable callable `side_effect(*args)` writes bytes to the real supplied errpipe and returns 0. A nested `patch("subprocess.os.waitpid", side_effect=ChildProcessError)` raises the exception class.
* The four POSIX `test_communicate_BrokenPipeError_stdin_*` methods patch `proc.stdin` and use independently cached child mocks for `write/flush/close/fileno`, assign exception-class `side_effect=BrokenPipeError`, set `fileno.return_value` to a real descriptor, and use `assert_called_with()` and `assert_called_once_with()` / `(b"stuff")`.
* POSIX `test_stopped` needs a configured tuple return from `subprocess.os.waitpid`; `test_send_signal_race` uses `patch("os.kill")` and `assert_not_called()`; `test_send_signal_race2` replaces one instance's `poll` by a lambda; `test_vfork_used_when_expected` uses `@patch("subprocess._USE_POSIX_SPAWN", new=False)` under the upstream strace decorator.
* All three `MiscTests.test_*keyboardinterrupt_no_kill` IDs call the decorated helper. It patches `Popen._communicate`, `Popen._wait`, then the module's `Popen` with its recording subclass. Calls raising `KeyboardInterrupt` must still enter `call_args_list` and `call_count`. It slices and iterates that list, compares entries to `mock.call(timeout=None)` and `mock.call(timeout=0.25)`, and formats the call list as an assertion message.

[Original source](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_subprocess.py). Subprocess owns whether native operations really dispatch through those targets; a mock applied only to a CPython proxy does not validate the native operation.

[Handoff to the unittest owner](https://github.com/astral-sh/imre/pull/508#issuecomment-6392208717).

## Recheck with the shared native Mock

The original import now executes at both optimization levels on [#758](https://github.com/astral-sh/imre/pull/758).
Its recorder asserted the actual native parent's waitpid call in
[the stopped-child fixture](STOPPED.md). A direct build of the original
`with mock.patch('subprocess.os.waitpid', return_value=(proc.pid, status))`
still stops at `mock.patch`. The original context and its tuple ran in CPython;
both the restored host function identity and a fresh native kernel wait were checked.

With #758 alone, the complete import and runner stop in `test.support` at
`warnings` (all definitions) or `_imp` (reachable definitions).
With #758 and merged main at `e90462e6`, both modes reach
`test/support/import_helper.py:2`, `import _imp`. These checks were repeated
as real builds at `-O0` and `-O3`; none entered a suite body.
[Compiler diagnostics and reference provenance](mock-startup-linux.json).
The full CPython rerun matched the committed 353-ID reference byte for byte.
