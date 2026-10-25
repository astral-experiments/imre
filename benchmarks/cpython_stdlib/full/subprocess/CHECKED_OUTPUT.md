# Native checked output

[native_checked_output.py](native_checked_output.py) runs six complete, unchanged
`ProcessTestCase` methods: `test_check_output_stderr`, `test_check_output_input_arg`,
and the four `test_check_output_input_none*` methods. The subprocess operation,
body and assertions run in Imre. CPython supplies module, class and instance
fixtures and the literal Python child programs. A native substring assertion
adapter compares typed string or byte slices and uses the native unittest
assertion to report failure. The input fixture supplies nonempty inherited
stdin to each case, so the original assertions can distinguish an explicitly
empty pipe from inherited stdin.

Run the script with the pinned interpreter, passing `--imre`, `--runtime`,
`--clang`, `--opt-level 0` or `3`, and `--result`. Both levels passed all six
complete methods. They are additional observations after the older module
inventory; they do not constitute a new full-suite count.

The [process differential](../../../../tests/fixtures/processes/checked_capture.py)
also compares checked status, original command, captures, invalid arguments,
UTF-8 error handling, and partial timeout output. At the point it catches
`TimeoutExpired`, an independent POSIX process tests the original PID with
`kill -0`; the child must already be reaped. The source module and test
expectations in the CPython fixture are unchanged.
