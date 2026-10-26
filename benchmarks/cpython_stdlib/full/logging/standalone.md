# Directly constructed loggers

`logging.Logger(name, level)` creates a fresh logger, even if another logger
has the same name or the name is `"root"`. Its parent is initially `None`.
Its level, propagation flag, cache and live handler list belong to the instance.
A managed logger with the same name still follows its own parent chain.

CPython’s [`LoggerTest.test_has_handlers`](../../upstream/Lib/test/test_logging.py)
adds a recording handler in setup. The unchanged method checks for a handler,
removes it while iterating the public list, and checks again. At O0 and O3 the
method, both assertions, and the logger operations run in Imre. The original
`LoggerTest` and `BaseTest` setup, teardown and cleanups run on CPython. The
native child recreates the root capture and the detached logger and uses the
upstream `RecordingHandler.handle` method. This is an assisted occurrence.

The [focused receipt](standalone-handlers-linux.json) also compares two
detached loggers and a managed logger with the same name. Each writes to the
correct stream; retaining a handler-list reference observes later removal.
The [full report](evidence-linux.json) retains all 272 occurrences per level.
This change updates only `LoggerTest.test_has_handlers`. The [cache method](cache.md)
retains its earlier measurement.

```sh
/path/to/cpython-3.14.5 -I -B benchmarks/cpython_stdlib/full/logging/native_standalone.py \
  --cpython-source /path/to/pinned/source --imre /path/to/imre \
  --runtime /path/to/libimre_runtime.a --clang /path/to/clang --result /tmp/standalone.json
```
