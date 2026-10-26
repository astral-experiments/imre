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
The replay covers logger hierarchy, stream delivery, record fields, handler registration,
formatters, and ordinary, timed and [size-based rotation](rotation.md). The counts agree at both levels.

The [standalone logger comparison](standalone.md) measures the original handler-removal loop.

The [manager cleanup comparison](manager-cleanups-linux.json) exercises both original enablement methods and their deferred restorations.
CPython runs both test bodies, their assertions and the original unittest cleanup stack.
Imre reads and writes the logger and manager state, including both deferred restorations.
The [direct manager comparison](manager_values.py) covers shared identity, private manager roots and thresholds, and cache behavior.
CPython also accepts assignment to `logger.manager`. Imre rejects that assignment at compile time.

| Body | Assertions | Logging operations | Fixture | Occurrences |
| --- | --- | --- | --- | ---: |
| Imre | Imre | Imre | CPython hooks and native child | 23 |
| Imre | CPython output check | Imre | CPython hooks and native child | 4 |
| CPython | CPython | Imre | CPython | 39 |
| CPython | CPython | Imre | CPython hooks and native child | 4 |
| Blocked before execution | | | | 199 |
| Unchanged reference skip | | | | 3 |

No occurrence has a fully native fixture. Native assertions check root logger
identity, immediate child membership, stopped handler propagation, level-name mapping independence, daily
rollover with an explicit time, record representation, formatter precision,
an empty name filter, both buffering formatters, and [FileHandler](files.md)
delayed creation and emission after closing a file opened in write mode.
Four additional unchanged rotation bodies check actual files and rollover decisions.
The [CP1251 StreamHandler method](stream-delivery.md) checks the encoded bytes and newline.
The stream replacement method checks sys.stderr, the old stream, and the unchanged-stream return.
Two original root configuration methods check the formatter, handler replacement and exact output before and after force.
Two more [filename configuration methods](config.md#filename-configuration) check native option conflicts and the UTF-8 bytes emitted through the root. The option-conflict assertions run in Imre; CPython checks the emitted file.
FileHandler also routes records through the live handler list. The 25 file-delivery observations add zero upstream test IDs.
Four bodies verify native stderr with the original
CPython output helper. The 43 methods with CPython bodies exercise [logger cache invalidation](cache.md), named handler registration, the registry, timed and rotating
files, record flags, [formatters](formatters.md), or [configuration](config.md).
The two [makeRecord methods](records.md#loggermakerecord-and-extra) check insertion and all 23 protected names. For insertion, the body and assertion run in Imre; for collisions, the original CPython loop and assertions drive a native child.
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
