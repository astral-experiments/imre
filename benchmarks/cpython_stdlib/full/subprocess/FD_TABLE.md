# Original POSIX descriptor-table tests

Four CPython methods run with their original assertions and fixtures. The fixture transfers the parent's open file descriptions into an Imre process at the original descriptor numbers, restores their close-on-exec flags, and checks both descriptor tables after the native parent reaps each child.

| Method | What the original test checks |
| --- | --- |
| `test_close_fds` | Three children inherit all 11 original fds, none, and then exactly the eight in the keep set. |
| `test_pass_fds_inheritable` | Both pipe fds survive in the child. The non-inheritable fd keeps that flag in the parent. |
| `test_pass_fds_redirected` | The three borrowed stdio descriptors survive exec at both 0/1/2 and the original numbers. |
| `test_pass_fds` | Each of ten inheritable fds reaches one child on its own. The corresponding `subprocess.call` emits `RuntimeWarning` before starting the fixture's zero-return command. |

At O0 and O3 the native parent launched and reaped 25 children and emitted ten override warnings. Withholding the warning failed the original `assertWarns`. Promoting the warning to an error prevented the fork. The method bodies and assertions ran on CPython. [Paired results](fd-table-landed-parent-linux.json)

Use the pinned interpreter and strace to run [the fixture](fd_table_originals.py) at each optimization level. For the negative control, pass `--only test.test_subprocess.POSIXProcessTestCase.test_pass_fds --withhold-warning`. From this directory, reapply the four paired observations to the subprocess report with:

```sh
python3.14 -B fd_table_evidence.py --previous evidence-linux.json \
    --fd-table fd-table-landed-parent-linux.json --result /tmp/subprocess-evidence.json
```
