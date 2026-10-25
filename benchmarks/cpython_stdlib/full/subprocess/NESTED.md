# Executing the subprocess operation inside a child

[The complete process traces](nested-chain-linux.json) identify the outer and
middle Imre executables and the final data writer by their successful execs.
The outer parent created and reaped the middle process once; the middle parent
created and reaped the writer once. All six original bodies and assertions ran
on CPython and passed at O0 and O3. Each normal outer parent made two `poll`
calls; each NoPoll outer parent made two `pselect6` calls. The fixture verifies
the binding installed by the original setUp and transfers it to the native
outer parent. The middle process and writer each start with a fresh binding.

[nested_parents.py](nested_parents.py) takes the two literal `-c` programs from
each original method. It compiles the exact UTF-8 source and installs a fixture
executable which accepts only those two arguments. The native launcher still
executes the selected interpreter and passes the original argument vector.
An exec trace must identify three different PIDs for the outer Imre parent,
the inner Imre program which itself calls subprocess, and its data child.

At O0 and O3, the ordinary `test_stdout_none` and
`test_stderr_redirect_with_no_stdout_redirect` have native operations in all three
processes. For `test_stdout_filedes_of_stdout`, the inner Imre parent makes the
asserted `subprocess.call(..., stdout=1)`; only the data child's
`os.write(sys.stdout.fileno(), ...)` runs on CPython. The six original case bodies,
assertions, and registered cleanup callbacks ran in the CPython fixture. Attempts
to compile their complete methods are recorded individually.

The [first run](nested-linux.json) found that the outer native parent kept
using poll in the three NoPoll cases despite correct output. The
[selector rerun](nested-select-linux.json) verified the binding after #682.
The current paired evidence uses the separate parentage and reap checks on
`44030132616437faa91f9fd2edebda8ff806bd6f`. These are six of the 353 original
occurrences.

Reproduce on Linux with strace and the pinned CPython:

```sh
python benchmarks/cpython_stdlib/full/subprocess/nested_parents.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --opt-level 0 --result /tmp/nested-0.json
```

Repeat with `--opt-level 3`. Use `nested_evidence.py --previous evidence-linux.json
--nested-chain nested-chain-linux.json --result combined.json` in this directory
to apply just these six rows.
