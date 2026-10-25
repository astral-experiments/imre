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
`test_pass_fds` also requires a RuntimeWarning and is measured separately below.
The [earlier run](fd-table-linux.json) and [hosted replay](fd-table-hosted-linux.json) retain their original toolchains. The current [paired replay](fd-table-replay-linux.json) used #762 at `1fae763` (tree `0c202539fb49b592bd5abc7df9af57240586381a`). Each optimization level changed exactly those three rows over canonical main `0e602ec`; the stopped-child result from #825 is unchanged.

Run `fd_table_originals.py --imre ... --runtime ... --clang ... --opt-level
{0,3} --result ...` with the pinned interpreter and strace. Use
`fd_table_evidence.py --previous <previous-report> --fd-table fd-table-replay-linux.json
--result evidence-linux.json` to reapply the observations to the complete
353-occurrence reports without replacing evidence for other methods.

## Overriding close_fds

[The fourth original method](pass-fds-warning-linux.json) ran on #1000 at
`28787b0` (tree `00b996d48af3fdf778c76e1e9f18e2ed02e0b292`). At each
optimization level the host's unchanged `test_pass_fds` opens five pipes and
marks all ten fds inheritable. For each original fd, a native Popen launches
the pinned Python `fd_status.py` child. The method checks that exact fd is in
the child's output and the other nine are absent.

For each fd the method also calls `subprocess.call` with the module fixture's
original tuple (the system's `true` executable). The compiled parent applies
the original `assertWarns` filter, writes the warning, starts the child, and
reaps it. The driver checks that order in the parent's syscall trace, transfers
the observed category, message and location to the host's real warning capture,
and returns the compiled call's actual status. All 20 children were reaped;
every parent fd number, target, and CLOEXEC flag matched the pre-launch table.
This is one host-driven native operation. The body, assertions and fixtures
remain on CPython; neither data child is an Imre process.

Run `fd_table_originals.py` with the tool/level flags above and
`--only test.test_subprocess.POSIXProcessTestCase.test_pass_fds`. Repeat with
`--withhold-warning` for the negative control: the same native parent emits
the warning, but the original assertion fails with “RuntimeWarning not
triggered” when the transport withholds it. The evidence generator accepts
the fourth observation only when the pair counts, native warning, descriptor
identities, reaps, child programs and negative control agree.

### Shared warning hierarchy replay

The [current four-method run](fd-table-shared-warning-linux.json) uses
[#1000](https://github.com/astral-sh/imre/pull/1000) at `d330a2b`
(tree `50a62b358800449aac1a8e0dde3b03b451d87c7e`), including the standalone
[Warning and RuntimeWarning base](https://github.com/astral-sh/imre/pull/1036)
at `a629bbd`. Both optimization levels ran all four original methods. The
unchanged `test_pass_fds` observes ten real warnings and twenty reaps per
level; withholding the native warning fails its original assertion. The other
three methods still account for three, one, and one successful native child
launches. Each keeps its host body, assertions, and fixture classification.
The four hierarchy catch controls are recorded separately from the method
results. Rebuild the pair with `fd_table_evidence.py --previous <#778-report>
--fd-table fd-table-shared-warning-linux.json --result evidence-linux.json`.
