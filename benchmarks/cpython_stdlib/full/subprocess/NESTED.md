# Executing the subprocess operation inside a child

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

The original output assertions also passed in the three NoPoll cases, but the
outer Imre parents called poll. Those occurrences remain blocked until the
native parent honors the host fixture's SelectSelector binding. The Linux
[trace summary](nested-linux.json) records the outcome, both execution engines,
original method and program hashes, and syscall counts per generation on subprocess
`1012f2f189784d935cde3a7f4190ea559726b432`. These are six of the 353 original
occurrences; the observations are supplemental, not extra tests.

Reproduce on Linux with strace and the pinned CPython:

```sh
python benchmarks/cpython_stdlib/full/subprocess/nested_parents.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --opt-level 0 --result /tmp/nested-0.json
```

Repeat with `--opt-level 3`.
