# POSIX user and group IDs

The [Linux results](credential-children-linux.json) cover 14 distinct release methods at both `-O0` and `-O3`. Unittest runs the original platform and privilege guards. Each kernel operation runs in an isolated process; a successful setter never changes the process running the test suite.

| Original methods | Body and assertions | OS operations |
| --- | --- | --- |
| `test_os.PosixUidGidTests.test_setreuid_neg1` and `test_setregid_neg1` | Imre runs the complete subprocess method; there are no explicit assertions. CPython supplies the fixture. | Imre executes the original `-c` program. A launcher verifies its source and arguments. |
| `test_posix.PosixTester.test_getresuid`, `test_getresgid`, `test_setresuid`, `test_setresgid` | Imre runs the four complete methods and all 12 assertions. CPython supplies setup and cleanup. | Imre calls POSIX directly. The setters request the process's existing IDs. |
| Six `PosixUidGidTests` error methods and the two `PosixTester` saved-ID error methods | CPython runs the complete original method and its callable assertions. | All 32 calls in the unprivileged Linux run execute in native children. Each row includes its call trace and the first compiler error for the complete body. |

Both native module names, `os` and `posix`, expose the real and effective ID getters and setters and the paired setters. Linux also exposes the saved-ID getters and setters. The OS calls preserve errno from libc and accept Python integer-index arguments.

The [runner](credential_children.py) additionally compares [argument conversion](credential_arguments.py) with CPython in separate disposable processes. It checks evaluation order, overridden and failed `__index__`, signed and unsigned boundaries, nullable and variable input types, positional arity and keyword errors. The matching output hash and line count are separate from the 14 upstream test IDs.

Two [supplemental comparisons](credential_boundaries.py) retain their own outcomes in the same artifact. Reading an exception's `__context__` blocks compilation. Returning a boolean from `__index__` produces the same conversion result, but misses the CPython deprecation warning; this comparison fails. Neither affects the denominator or counts as a native pass.

Run `credential_children.py` with the release interpreter, `--cpython-source`, `--imre`, `--runtime`, `--clang`, `--opt-level 0|3`, and `--result`. Fold the two outputs with `evidence.py --credential-results O0.json O3.json`. The [callable assertion diagnostics](https://github.com/astral-sh/imre/pull/587#issuecomment-6393798475) are tracked with the unittest owner.
