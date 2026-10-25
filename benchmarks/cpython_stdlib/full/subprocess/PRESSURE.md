# Descriptor pressure in the original Popen parent

Both `test_no_leaking` fixtures open files until EMFILE, close ten of them,
and launch 15 children in succession with stdin, stdout and stderr piped.
The earlier adapter created a fresh native process plus control pipes on every
call and retained those pipes in the host. This obscured what leaked.

The [new driver](pressure_fixture.py) starts one native parent. It receives all
1,011 original open file descriptions at their original numbers, restores their
CLOEXEC flags, and verifies its entire descriptor table and soft limit against
the fixture before launching any child. It uses stdin/stdout to control the
parent. After **each** original communicate it checks that every parent fd still
has the same number and target. Independently, pid-specific traces identify
every successful CPython data-child exec and the same native parent's 15 waits.

The [O0 and O3 measurements](pressure-linux.json) passed both upstream bodies
and their assertions on the host. The ordinary native parent used poll and the
NoPoll parent used pselect6. This is evidence for two host-driven native
operations per level. The recorded build combines [the selector implementation](https://github.com/astral-sh/imre/pull/682)
with main `fce4734`; these counts do not imply fully native fixtures.

The [repeat on the later selector compiler](pressure-replay-linux.json)
checks all 15 leak-free launches in each variant again and attempts each full
method at O0 and O3. The raw result includes its compiler diagnostic alongside
the observed native operation. The native ordinary parent made 47 poll calls at
O0 and 45 at O3; each NoPoll parent made 46 pselect6 calls. These counts vary
with scheduling. All four parents reaped every child and preserved the fd table
after each communication. The two records change from failed to host-driven
native operations on main `41ea878`; the other 351 records are unchanged.
The measured source was #682 at `12a645f` (tree
`3d7be7793cd60f833814e5fb0ebdb7e23d2f4c26`). The
[preceding selector replay](pressure-earlier-selector-linux.json) retains its
original toolchain.

Run the pinned interpreter with `pressure_fixture.py --imre ... --runtime ...
--clang ... --opt-level 0 --result ...`, then repeat at O3 on Linux with
`strace` and procfs. `pressure_evidence.py PREVIOUS RESULT` updates only
these two rows and validates all 353 occurrences at both levels.
