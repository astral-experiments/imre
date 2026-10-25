# CP1251 codec writers

The assertion in CPython 3.14.5's
`test.test_logging.EncodingTest.test_encoding_cyrillic_unicode` compares twelve
bytes: `e4 ee 20 f1 e2 e8 e4 e0 ed e8 ff 0a`. The last byte is the newline
written by the stream handler. The [driver](codec_writer.py) runs the original
method with its original setup and teardown under the pinned interpreter and
records both operands of its real assertion.

At O0 and O3, the [native stream fixture](../../../../tests/fixtures/files/codec_writer.py)
writes the same twelve bytes through `codecs.getwriter('cp1251')` to a real
`io.BytesIO`. The fixture explicitly supplies the newline. It compares alias
identity, class and instance attributes, error policies, atomic writes, saved
buffer snapshots, and closed streams against CPython. A second native program
encodes and decodes all 255 defined CP1251 bytes and checks the remaining byte,
`0x98`, against four error policies.

The writer uses the [sys codec registry](https://github.com/astral-sh/imre/pull/691)
and the charmap already used to decode Python source. These two native programs
exercise the writer directly; they add zero occurrences to the logging suite.
The [receipt](codec-writer-linux.json) records the reference method separately.

Run with the pinned CPython 3.14.5 interpreter:

```sh
python -I -B benchmarks/cpython_stdlib/full/logging/codec_writer.py \
  --cpython-source /path/to/cpython --imre /path/to/imre \
  --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --result /tmp/codec-writer-linux.json
```

The production compiler and the compiler built with IR verification reproduced
the [writer receipt](codec-writer-linux.json) at O0 and O3 with the merged
[sys registry](https://github.com/astral-sh/imre/commit/796917bfff5c13cfbf7a2f219c9d08608e6f8fb8).
Both builds matched CPython's subprocess text output and writable
`Popen.returncode` fixtures. The registry's ownership unit test and its
[device receipt](../sys/codec-device-linux.json) passed. The separate
[StreamHandler run](https://github.com/astral-sh/imre/actions/runs/40217698211)
covers the handler adding the newline.
