# POSIX child finalizers

The two unchanged CPython 3.14.5 methods run their original warning capture,
assertions, teardown and registered pipe cleanups. The fixture translates each
Popen request, `id`, list observation, pipe close and wait to the compiled parent.
The original child argument vector still launches pinned CPython. A C fixture
implements only `kill` and `waitpid` for the original test's `os` calls; the
Imre runtime implements Popen and the active-child list.

| Original method | Observed in the native parent at O0 and O3 |
| --- | --- |
| `test_zombie_fast_process_del` | The warning names the child's PID. The original Popen remains in the list, teardown waits on that same object (exit 0), and explicit cleanup empties the list. |
| `test_leak_fast_process_del_killed` | SIGKILL succeeds. The next launch reaps the child before attempting the missing executable (ENOENT). A later waitpid returns ECHILD and the list is empty. |

Linux procfs identifies the parent executable and strace checks the child's
arguments, successful reap count, and both descriptor closes. A host object
forwards the warning actually emitted by the native parent into the original
warning recorder. The complete body attempt still stops at the `test` package
import. Both observations are **host-driven native operations** with mixed
fixtures. Neither the Python data child nor the unittest body counts as native.

With the pinned interpreter, compatible Imre binary/runtime, clang and strace:

```sh
python active_originals.py --imre IMRE --runtime RUNTIME --clang CLANG --opt-level 0 --result raw-0.json
python active_originals.py --imre IMRE --runtime RUNTIME --clang CLANG --opt-level 3 --result raw-3.json
python active_evidence.py --previous evidence-linux.json --active-children active-children-linux.json --result combined.json
```

[active-children-linux.json](active-children-linux.json) records the tested Git
tree, hosted artifact, compiler/runtime hashes, and level-specific native parent
hashes alongside the two observations.
