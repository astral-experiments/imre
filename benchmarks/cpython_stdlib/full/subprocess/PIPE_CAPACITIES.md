# `Popen(pipesize=…)` on Linux

[The paired observations](pipe-capacities-linux.json) cover `test_pipesize_default`
and `test_pipesizes` under both `ProcessTestCase` and `ProcessTestCaseNoPoll`.
At each optimization level they account for four of 353 occurrences. The runtime
change is [#906](https://github.com/astral-sh/imre/pull/906).

| Original request | stdin, stdout and stderr capacity | Native parent before spawn |
| --- | --- | --- |
| `-1` | 65,536 bytes per pipe, as in the fixture’s `os.pipe()` | No `F_SETPIPE_SZ` |
| Half the default | 32,768 bytes per pipe | One successful `F_SETPIPE_SZ` per pipe |

The unchanged host assertions use real `fcntl(F_GETPIPE_SZ)`. The fixture
reopens each native pipe via `/proc/<native pid>/fd`, verifies the inode and
releases its temporary descriptor immediately after the query. Capacity belongs
to the kernel pipe. The temporary descriptor does no I/O. The native parent
writes `pear` and closes all three streams in the positive-size case; the native
Popen context closes them in the default case. Each parent calls `kill` and
reaps its child once. Neither routine calls a selector. The child is pinned
CPython running the original standard-stream payload.

The shared runner still stops at the original method decorator. Each row counts
as a **host-driven native operation**: the body and assertions run on CPython,
while Popen and its streams run in Imre. The fixture checks the platform’s
measured sizes; the upstream positive-size test can skip if halving the default
would leave less than a page.

Run on Linux with procfs and permission to trace the native process. Repeat at
`--opt-level 3`:

```sh
"$CPYTHON" -E -B benchmarks/cpython_stdlib/full/subprocess/pipe_capacity_originals.py \
    --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
    --opt-level 0 --result /tmp/subprocess-pipe-capacities-0.json
```
