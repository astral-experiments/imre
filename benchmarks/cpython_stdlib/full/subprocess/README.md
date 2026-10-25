# CPython 3.14.5 subprocess

[UPSTREAM.json](UPSTREAM.json) pins the unchanged test module, five child
programs from `Lib/test/subprocessdata`, and the PSF license to CPython commit
`5607950ef232dad16d75c0cf53101d9649d89115`. The test and license link to the
existing vendored copies. Use a test-enabled build of that release; its own
`Lib/test` supplies unittest, regrtest, and their support modules.

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
