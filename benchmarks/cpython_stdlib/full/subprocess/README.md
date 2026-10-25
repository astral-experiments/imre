# CPython 3.14.5 subprocess

[UPSTREAM.json](UPSTREAM.json) pins the unchanged test module, five child
programs from `Lib/test/subprocessdata`, and the PSF license to CPython commit
`5607950ef232dad16d75c0cf53101d9649d89115`. The test and license link to the
existing vendored copies. Use a test-enabled build of that release; its own
`Lib/test` supplies unittest, regrtest, and their support modules.

The current [per-occurrence reports](evidence-linux.json) distinguish the test
body, assertions, subprocess operation and fixture at each optimization level.
[Pipe capacities](PIPE_CAPACITIES.md) describes how the original `fcntl`
assertions inspect the compiled parent's stdin, stdout and stderr.

## Linux reference

[reference-linux.json](reference-linux.json) records **all 353 distinct IDs**.
The release's unittest loader and regrtest agree: 312 passed, 41 skipped with
their original reasons. The CPU resource is disabled in this reference run;
those two skips are not a platform exclusion. The run used an unprivileged
Linux account outside a user namespace. Inside the devbox sandbox, changing to
an unmapped uid/gid yields EINVAL in eight subtests of `test_user` and
`test_group`; these are not eight additional test IDs.

| Test class | IDs |
| --- | ---: |
| CommandsWithSpaces | 4 |
| ContextManagerTests | 5 |
| MiscTests | 5 |
| POSIXProcessTestCase | 78 |
| ProcessTestCase | 110 |
| ProcessTestCaseNoPoll | 110 |
| RunFuncTestCase | 21 |
| Win32ProcessTestCase | 20 |

From the repository root, with the shared inventory runner from [#479](https://github.com/astral-sh/imre/pull/479):

```sh
"$CPYTHON" benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/subprocess/UPSTREAM.json \
    --result /tmp/subprocess-reference.json
```

## Native baseline

At both `-O0` and `-O3` on Imre `eafdbda`, building the original suite stops
at line 1, `import unittest`. No entire test body runs in Imre. The two existing
drivers measure different operations:

- [native_subprocess.py](../../native_subprocess.py) runs eight unchanged
  `RunFuncTestCase` methods in host unittest: `test_capture_stdout`,
  `test_capture_stderr`, `test_capture_output`, `test_check`, `test_check_zero`,
  `test_timeout`, `test_check_output_timeout`, and `test_stdout_stdout`.
  A compiled Imre **parent** launches the requested command and returns its
  status, captured bytes, or exception. Host code transports the result and
  executes assertions. All eight passed at each level.
- [native_sys.py](../../native_sys.py) compiles the **child** for
  `ProcessTestCase.test_communicate_stdout`, `ProcessTestCase.test_communicate_stderr`,
  `RunFuncTestCase.test_returncode`, and `RunFuncTestCase.test_check`. The
  host performs launch, communicate, and status checking. These are child
  runtime observations; they do not verify Imre's subprocess implementation.
  `test_check` also has independent native-parent evidence above.

Across the 353 IDs, the baseline is 0 fully native, 8 host-driven native
operations, 3 additional mixed/child-only, 301 reference-only, and 41 reference
skips. The whole-file compile blocker applies to all IDs; it is not an extra
row in the denominator. The mixed-stream and sys exit fixtures in the drivers
are not upstream test IDs. No sibling's PR is included: #302 concerns child
CLI execution, and #405/#409 concern standard-stream errors and finalization.

## Live process handles

[native_bodies.py](native_bodies.py) compiles the complete, unchanged
`ContextManagerTests.test_returncode` method with a native assertion adapter.
Its `Popen`, context exit, wait, and status observation execute in the Imre
parent; the Python child exits with 100. Both optimization levels executed
the assertion. The original fixture runs on CPython, so this is one native
body with a host fixture. Reproduce at levels 0 and 3 using:

```sh
"$CPYTHON" benchmarks/cpython_stdlib/full/subprocess/native_bodies.py \
    --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
    --opt-level 0 --result /tmp/subprocess-native-0.json
```

Running regrtest again with `-u cpu -m '*test_leaking_fds_on_error'` passes
both previously gated IDs on CPython, each performing 1,024 failed launches.
They remain eligible for native execution. This supplemental run does not
change the results of the default reference command.

## Pipes and communication

The same native-body command now executes 11 unchanged methods (the keys
under `native_body_host_fixtures` in its result) at both levels. It covers stdin writes,
stdout/stderr reads, merged output, EOF/closed state, and `communicate` with
zero or one pipe. `ProcessTestCase.test_communicate_stdout` and
`test_communicate_stderr` now have native-parent evidence as well as the older
child-only observation. The native body of
`test_post_timeout_communicate_sends_input` is attempted and reported under
`blocked`: the compiler does not infer that its exception handler calling
`self.fail()` cannot return. The original body is not rewritten.

The differential `tests/fixtures/processes/popen_pipes.py` fixture also checks
a 200,000-byte exchange on all three pipes, recovery after a zero deadline,
rejection of new input, repeated output, text newline conversion, and shared
descriptor/close state. It is not another upstream ID.

## Original fixtures and per-ID evidence

Use the shared [suite runner](../../SUITE_RUNNER.md) from
[#508](https://github.com/astral-sh/imre/pull/508) to run these bodies under
the pinned release's module, class, and instance hooks. In
`native_body_host_fixtures`, the body, assertions, and subprocess operation
execute in Imre; the fixtures execute in CPython. The literal Python helpers
launched by the methods still execute in the specified CPython interpreter.

[host_popen.py](host_popen.py) runs original host bodies and assertions, with a
persistent Imre parent performing the requested Popen operations. The transport
returns the native status, stdout, stderr, and exception message. In particular,
the post-timeout test performs both communicate calls on the same native child.
It records which calls actually ran. [audit.py](audit.py) exercises this route
in isolation for every eligible reference ID, with the CPU resource enabled.
A successful host assertion with no native operation, a subprocess operation
executed inside a CPython child, or a check of the wrong process's descriptor
table is not a native subprocess pass.

Run either script with the same flags as `native_bodies.py`. The latter's
`blocked` and the audit's failed/blocked rows remain in the denominator.

[native-linux.json](native-linux.json) records the strongest evidence for each
of the **353** IDs on the pipes implementation. Both levels produced the same
category totals. Repeated operations, including timeout retries, retain their
separate counts if the levels differ.

| Execution of the tested operation | -O0 | -O3 |
| --- | ---: | ---: |
| Native body, assertions, and subprocess operation; original host fixtures | 11 | 11 |
| Host body and assertions, verified native subprocess operation | 36 | 36 |
| Mixed (including a subprocess operation inside a CPython child) | 8 | 8 |
| Reference-only | 6 | 6 |
| Blocked | 227 | 227 |
| Failed | 26 | 26 |
| Original reference skip, with individual reason | 39 | 39 |
| **Total** | **353** | **353** |

There are zero end-to-end native unittest fixture executions. Eight of the 36
host-driven IDs use the older `native_subprocess.py` reconstructed host case;
the other 28 use the original CPython fixtures. String argument sequences are
transported by value to the native launcher. This route does not establish
tuple identity, ownership of inherited host descriptors, or a native
`SelectSelector` when the NoPoll fixture patches the host module. The Linux
reference passes the strace case outside the devbox sandbox. An unavailable
worker fixture is a blocker rather than a new reference skip.

The host can verify a closed pipe after a native context exit raises an
`OSError`. The control process continues using that same native handle to
check the reaped status. Arguments containing NUL travel in separate files:
both invalid commands reach Imre instead of failing at the host's exec call.

To regenerate the report, run `audit.py`, `native_bodies.py`, and
`../../native_subprocess.py` for each level, capturing the latter's combined
output. Pass the two files from each command, in level order, to
`report.py --audit O0 O3 --bodies O0 O3 --run-log O0 O3 --imre-revision REV --result FILE`.

## Per-occurrence evidence on Linux

[evidence-linux.json](evidence-linux.json) follows the [shared evidence contract](../../NATIVE_EVIDENCE.md).
It covers the 353 original loader occurrences (353 distinct IDs) at each of -O0 and -O3.
The [selector replay](CHECKED_AND_PIPES.md) measures 25 rows on Imre `4851037`;
other rows retain their individual measured revisions. There is no companion Python test
package: `test.test_subprocess` is one module, and the five `subprocessdata` scripts are
child programs. The earlier [native-linux.json](native-linux.json) records a different
pipe implementation revision and remains useful for comparing individual calls.

| Execution | -O0 | -O3 |
| --- | ---: | ---: |
| Fully native | 0 | 0 |
| Native method and subprocess operation, CPython fixture | 54 | 54 |
| Host method and assertions, native subprocess operation | 95 | 95 |
| Both host and native subprocess operations | 3 | 3 |
| Host subprocess operation only | 6 | 6 |
| Blocked | 148 | 148 |
| Failed | 6 | 6 |
| Upstream skip with exact condition and source (2 CPU cases pass when enabled) | 41 | 41 |
| **Total** | **353** | **353** |

[attempt_bodies.py](attempt_bodies.py) tried every eligible complete body via the shared
runner, including the methods inherited by the NoPoll class. The native method source
and its helpers are taken byte for byte from the pinned file. The oracle supplies the
original fixtures, selected module globals and literal Python child programs. The native
parent launches the child, owns the file descriptors, communicates, waits, and propagates
the result. Native status does not imply that the child is native. Twelve inherited
NoPoll cases never reached a selector in the native parent; [selector_proof.py](selector_proof.py)
checked each one, with a separate timed communication case as a positive control.
The control has no separate assertion call: its original loop must catch timeouts and
eventually return. The ledger identifies any host-driven cases that use the older
reconstructed `RunFuncTestCase` fixture. Six cases exercising the reference's private process interface
are explicitly recorded as host operations. See [MOCK.md](MOCK.md) for the shared mock requirements.

The default reference disables the CPU resource. The two inherited `test_leaking_fds_on_error`
occurrences include a separate `evidence.cpu_enabled` observation. With CPU enabled, the
original reference and host-driven Imre operations both pass. Each original host method
issued all 1,024 failed launches to one Imre parent. The parent had the same five open
descriptors before and after, at a soft limit of 64. Its syscall trace recorded 1,024 child
creations and 1,024 reaps, with no selector calls, at both levels. These results do not
add occurrences or upgrade the CPython fixtures, loop, or assertions.

To reproduce, run `attempt_bodies.py` and `audit.py` with the pinned Python, Imre
compiler and runtime, and `--opt-level 0` or `3`. Run the original vfork fixture separately
with `attempt_bodies.py --only test.test_subprocess.POSIXProcessTestCase.test_vfork_used_when_expected`
where strace is permitted. Run `selector_proof.py --mode build`, followed by `--mode trace`
with the same tools and work directory. The trace checks compiler, runtime, binary and method
hashes; it traces the native parent only. Also run `../../native_subprocess.py` for each
level and capture its combined output. For the CPU cases, use `leak_cleanup.py --mode build`
and then `--mode exercise` with identical `--imre`, `--runtime`, `--clang`,
`--opt-level`, `--work`, and `--result` arguments. The latter needs Linux procfs and strace.
Combine those files and validate from the repository root:

```sh
python benchmarks/cpython_stdlib/full/subprocess/evidence.py --audit AUDIT0 AUDIT3 --bodies BODY0 BODY3 --trace TRACE0 TRACE3 \
    --vfork VFORK0 VFORK3 --run-log LEGACY0 LEGACY3 --cpu CPU0 CPU3 --imre-revision FULL_SHA --result benchmarks/cpython_stdlib/full/subprocess/evidence-linux.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/subprocess/reference-linux.json \
    benchmarks/cpython_stdlib/full/subprocess/evidence-linux.json
```

The [paired checked-command and pipe runs](CHECKED_AND_PIPES.md) include a
complete attempt for every occurrence, both selector fixtures, and a separate
trace for operations whose completion alone would not establish compatibility.

[Stopped child status](STOPPED.md) distinguishes a real kernel stop from the
unchanged test's waitpid mock and records both optimization levels.

### Raw pipe readiness

[The original Linux readiness test](raw-pipe-readiness-linux.json) runs unchanged
under the pinned CPython fixtures at O0 and O3. Imre launches the original data
child, requests exactly four bytes from the raw pipe, returns `b'appl'`,
closes the pipe through the fixture's cleanup, and reaps exit zero. Host
`select.select` watches a descriptor for that same kernel pipe and returns the
original stream. This occurrence is mixed: the body, assertions, and readiness
check run on CPython.

With the writer kept alive, the raw read leaves one of the five bytes ready
in the kernel; default buffering fetches all five and host select reports no
readiness. Both return the same four bytes to the caller. The earlier “int, or
have a fileno()” failure came from the host adapter. The complete native body
still stops at importing upstream test support.

Run `readiness_original.py --imre ... --runtime ... --clang ... --opt-level {0,3}
--result ...` with the pinned interpreter, Linux procfs and strace. Apply the
pair with `readiness_evidence.py --previous <baseline> --readiness
raw-pipe-readiness-linux.json --result evidence-linux.json`.
