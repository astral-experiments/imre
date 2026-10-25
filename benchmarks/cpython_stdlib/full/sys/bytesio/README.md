# BytesIO snapshots for codec streams

CPython's `test.test_logging.EncodingTest.test_encoding_cyrillic_unicode` supplies an `io.BytesIO` to the codec writer and inspects the resulting bytes with `getvalue()`. This directory checks that prerequisite independently of the writer, logging handler and sys denominator.

`verify.py` verifies the upstream `Lib/test/test_memoryio.py` blob against both the licensed vendored copy in `full/doctest/upstream` and the supplied CPython 3.14.5 checkout. It discovers the original suite, tries to compile the entire `CBytesIOTest.test_getvalue` method, and records the result at both optimization levels. If compilation stops, the original CPython method drives a live native stream through the shared stream fixture. The receipt distinguishes that execution from a native body.

The independent `snapshots.py` program compares byte identity, contents, cursor and alias behavior, and closing with stock CPython. `ordinary_file.py` verifies that an ordinary binary file has no snapshot method.

Run with the pinned CPython binary:

```sh
python -B benchmarks/cpython_stdlib/full/sys/bytesio/verify.py \
  --cpython-source /path/to/cpython --imre /path/to/imre \
  --runtime /path/to/libimre_runtime.a --clang /path/to/clang --result observations.json
```
