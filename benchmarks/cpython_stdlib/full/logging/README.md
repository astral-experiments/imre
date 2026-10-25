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

[Linux native evidence](evidence-linux.json) records all 272 occurrences at
each of `-O0` and `-O3` using the [v1 contract](../../NATIVE_EVIDENCE.md).
The refreshed run includes the level registry, timed file handler, and LogRecord
coverage merged before this report. The counts below are identical at both levels:

| Body | Assertions | Logging operations | Fixture | Occurrences |
| --- | --- | --- | --- | ---: |
| Imre | Imre | Imre | CPython hooks and native child | 4 |
| Imre | CPython output check | Imre | CPython hooks and native child | 4 |
| CPython | CPython | Imre | CPython | 23 |
| Blocked before execution | | | | 238 |
| Unchanged reference skip | | | | 3 |

No occurrence is fully native. The four native assertion cases are root
logger identity, level-name mapping independence, daily rollover with an
explicit time, and LogRecord representation. The four output cases verify
emitted native stderr with the original CPython helper. The 23 host-driven
methods call a persistent native process for the registry, file handler, or
LogRecord metadata; setup and registered cleanup stay with their test. The report keeps original method locations, individual
compiler or adapter blockers, and the three pinned skip conditions. It
does not treat a compiled source or a passing CPython reference as a native pass.

```sh
/path/to/pinned/python -I -B benchmarks/cpython_stdlib/full/logging/evidence.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --result /tmp/logging-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
  benchmarks/cpython_stdlib/full/logging/reference-linux.json /tmp/logging-evidence.json
```

Reference execution does not count as Imre coverage. The existing
[native runner](../../native_logging.py) compiles whole methods in fresh
executables: three hierarchy tests delegate output comparison to the upstream
host assertion, and the root alias test checks identity in the executable.
It supplies replacement setup and assertion helpers; it does not execute
CPython's full TestCase lifecycle natively.

[Log record execution](records.md) lists the separate native-body and host-driven
coverage for record representation, metadata and flag changes.
