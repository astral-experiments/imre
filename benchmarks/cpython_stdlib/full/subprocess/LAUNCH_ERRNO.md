# Failed child launches

The seven original Linux occurrences in [launch-errno-linux.json](launch-errno-linux.json)
exercise four executable failures and three directory failures at each optimization.
The native parent forked a child, received its actual syscall error, reaped it, and
returned the exception fields. The original CPython methods and assertions checked
those values. No data program executed successfully. Both selector fixtures ran;
the failure preceded any selector syscall in every traced native parent.

The complete native method attempt is recorded separately for each occurrence.
Four methods carry the upstream Windows skip decorator; the shared native runner
requires support for that decorator before it can compile them. The three POSIX
methods include the upstream helper which obtains the comparison exception from
`os.chdir`. Its first unsupported expression is recorded as a compiler diagnostic.
These are host-driven native operations, with no fully native pass claimed.

Use the pinned interpreter to run `launch_errno_originals.py` with
`--imre --runtime --clang --opt-level {0,3} --result`. The driver compiles a
parent, verifies the test and fixture blobs, and checks the failed syscall and
its reap in distinct PID traces. Each error field is transported from the native
exception. A bridge that substitutes the input filename or fills in errno cannot
satisfy this check.

`public_popen_evidence.py --previous <earlier-evidence-linux.json>
--launch-errno launch-errno-linux.json --result evidence-linux.json` joins these
observations without changing other occurrences or the default reference. Both
reports retain 353 occurrences and the original 41 reference skips.
