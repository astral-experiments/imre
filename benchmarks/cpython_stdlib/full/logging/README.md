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
The replay of [`7a62075`](https://github.com/astral-sh/imre/commit/7a620758f5e244f26ddc3140eaca4d5b44246dcc)
includes logger hierarchy and handler propagation, the level registry, ordinary
and timed file handlers, records, formatters, filters and configuration.
Both levels have the same counts:

| Body | Assertions | Logging operations | Fixture | Occurrences |
| --- | --- | --- | --- | ---: |
| Imre | Imre | Imre | CPython hooks and native child | 16 |
| Imre | CPython output check | Imre | CPython hooks and native child | 4 |
| CPython | CPython | Imre | CPython | 35 |
| CPython | CPython | Imre | CPython hooks and native child | 4 |
| Blocked before execution | | | | 210 |
| Unchanged reference skip | | | | 3 |

No occurrence has a fully native fixture. Native assertions check root logger
identity, immediate child membership, stopped handler propagation, level-name mapping independence, daily
rollover with an explicit time, record representation, formatter precision,
an empty name filter, both buffering formatters, and [FileHandler](files.md)
delayed creation and emission after closing a file opened in write mode. The real FileHandler subclass also routes through the live handler registry; its 25 additional differential observations are outside this denominator. The
[CP1251 StreamHandler method](stream-delivery.md) compares the native writer’s
bytes, including the newline, under its original host fixture. The stream replacement
method checks the actual sys.stderr, the previous stream, and the unchanged-stream return. Two [root configuration methods](config.md) check the
attached formatter, handler replacement and exact output before and after force.
Four further original configuration bodies run on CPython. They check the native file handler, UTF-8 delivery, error ordering, and ASCII output with explicit ignore or default backslash replacement.
Four bodies verify native stderr with the original
CPython output helper. The 35 host-driven methods exercise named handler registration, the registry, timed
files, record flags, [formatters](formatters.md), or [configuration](config.md).
Setup and registered cleanup stay with their test. The report keeps original method locations, individual
compiler or adapter blockers, and the three pinned skip conditions. It
does not treat a compiled source or a passing CPython reference as a native pass.

```sh
/path/to/pinned/python -I -B benchmarks/cpython_stdlib/full/logging/evidence.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --result /tmp/logging-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
  benchmarks/cpython_stdlib/full/logging/reference-linux.json /tmp/logging-evidence.json
```

[Whole-suite startup](startup.md) records the first diagnostic for both native
entry points and isolates the queue configuration test's starred tuple.

Reference execution does not count as Imre coverage. The existing
[native runner](../../native_logging.py) compiles whole methods in fresh
executables: three hierarchy tests delegate output comparison to the upstream
host assertion, and the root alias test checks identity in the executable.
It supplies replacement setup and assertion helpers; it does not execute
CPython's full TestCase lifecycle natively.

[Log record execution](records.md) lists the separate native-body and host-driven
coverage for record representation, metadata and flag changes.

[Formatter execution](formatters.md) covers all three styles and records where
the original precision regression executes.
