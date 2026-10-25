# Original POSIX descriptor-table tests

The [three paired occurrences](fd-table-replay-linux.json) run the original CPython
methods, assertions, and hooks. The fixture transfers all the original open
file descriptions into the native parent at the original numbers and restores
their initial close-on-exec flags. The native Popen launches the original
`fd_status.py` data child and reaps it. The host's file table and the native
parent's table are checked independently.

| Method | What the original child and parent observe |
| --- | --- |
| `test_close_fds` | In three launches the child inherits all 11 original fds, none, then exactly the eight kept by the original set. |
| `test_pass_fds_inheritable` | Both pipe fds survive in the child. The original non-inheritable fd remains non-inheritable in the parent. |
| `test_pass_fds_redirected` | All three borrowed stdio descriptors survive at their original numbers as well as at 0, 1 and 2. |

At O0 and O3 the trace checks the native parent, its actual child exec and
wait, the child-only descriptor changes, and the unchanged parent descriptors
and flags. The native whole-body attempts still stop at importing upstream
test support. These are host-driven native operations. The different
`test_pass_fds` requires a RuntimeWarning and is not promoted by this report.
The [earlier run](fd-table-linux.json) and [hosted replay](fd-table-hosted-linux.json) retain their original toolchains. The current [paired replay](fd-table-replay-linux.json) used #762 at `1fae763` (tree `0c202539fb49b592bd5abc7df9af57240586381a`). Each optimization level changed exactly those three rows over canonical main `0e602ec`; the stopped-child result from #825 is unchanged.

Run `fd_table_originals.py --imre ... --runtime ... --clang ... --opt-level
{0,3} --result ...` with the pinned interpreter and strace. Use
`fd_table_evidence.py --previous <previous-report> --fd-table fd-table-replay-linux.json
--result evidence-linux.json` to reapply the observations to the complete
353-occurrence reports without replacing evidence for other methods.
