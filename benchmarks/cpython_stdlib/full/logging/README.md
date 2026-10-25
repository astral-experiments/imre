# CPython logging suite

[CPython 3.14.5](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test)
ships one logging test module. It exercises `logging`, `logging.handlers`
and `logging.config`. Unittest discovery includes inherited socket tests and
the conditionally defined queue listener tests. The complete Linux denominator
is **272 distinct IDs in 40 classes**.

[UPSTREAM.json](UPSTREAM.json) identifies the already vendored
[test module](../../upstream/Lib/test/test_logging.py) and [license](../../upstream/LICENSE).
The reference runner verifies those blobs in both trees and also verifies the
logging test's directly imported support modules and its HTTPS certificate in
the interpreter's source tree. Run from the repository root with a test-enabled
build of the pinned interpreter:

```sh
/path/to/python -B benchmarks/cpython_stdlib/full/logging/reference.py --result /tmp/logging.json
```

[Linux reference](reference-linux.json): **269 pass, three upstream skips** with
all resources enabled. Two multiprocessing configuration tests require a CPython
debug build; the NT event log test requires Windows modules. None of the local
TCP, UDP, Unix socket, IPv6, syslog, SMTP, or HTTP/TLS tests was excluded.
Every reference outcome retains its original ID and skip reason.

Reference execution does not count as Imre coverage. The existing
[native runner](../../native_logging.py) compiles whole methods in fresh
executables: three hierarchy tests delegate output comparison to the upstream
host assertion, and the root alias test checks identity in the executable.
It supplies replacement setup and assertion helpers; it does not execute
CPython's full TestCase lifecycle natively.
