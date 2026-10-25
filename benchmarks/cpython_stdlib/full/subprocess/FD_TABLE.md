# Original POSIX descriptor-table tests

The [three paired occurrences](fd-table-current-linux.json) run the original CPython
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
The [earlier run](fd-table-linux.json), [hosted replay](fd-table-hosted-linux.json) and [first paired replay](fd-table-replay-linux.json) retain their original toolchains.

The current receipt pairs O0 and O3 on #762 at `467c296`, based on main's actual #693 squash. Its `prospective_merge` field contains the same three complete observations on the actual merge with main `b69d36a` (tree `8821253d986697d1945460cb1029d6ad1437640c`). Both runs used the same fd numbers, original child arguments and host fixtures. Each produced three, one and one native child reaps. Applying either run to the canonical report changes only these three of 353 occurrences per level.

Run `fd_table_originals.py --imre ... --runtime ... --clang ... --opt-level
{0,3} --result ...` with the pinned interpreter and strace. Use
`fd_table_evidence.py --previous <previous-report> --fd-table fd-table-current-linux.json
--result evidence-linux.json` to reapply the observations to the complete
353-occurrence reports without replacing evidence for other methods.
