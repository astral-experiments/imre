# Native POSIX child settings

[The paired results](child-settings-linux.json) trace the three unchanged CPython
methods on main at `73aa817`, after [#693](https://github.com/astral-sh/imre/pull/693)
merged. At O0 and O3, the native parent applies the requested setting before exec
and reaps each child once:

| Original method | What the child observed |
| --- | --- |
| `test_start_new_session` | After `setsid()`, `getsid(0)` returns the child's PID, different from the parent's session ID. |
| `test_process_group_0` | After `setpgid(0, 0)`, `getpgid(0)` returns the child's PID, different from the parent's process group. |
| `test_umask` | With umask `0053`, the child's `O_CREAT` file has mode `0624`. |

CPython runs the original methods, assertions, fixtures and literal data
children. Imre runs the parent subprocess operations. Only these three of 353
records change at each level. Each retains its native-body diagnostic.

The paired results also include the replay on #693's published head
(`dee3402`). The [first local replay](child-settings-first-replay-linux.json)
and [earlier hosted replay](child-settings-hosted-linux.json) retain their
original source and toolchain identities.

Run `child_settings_originals.py --imre ... --runtime ... --clang ...
--opt-level {0,3} --result ...` with pinned CPython 3.14.5 and strace.
Use `child_settings_evidence.py --previous PREVIOUS --child-settings
child-settings-linux.json --result RESULT` to apply only these records.
