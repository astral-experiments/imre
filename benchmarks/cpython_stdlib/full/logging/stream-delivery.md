# StreamHandler delivery to a CP1251 writer

The complete pinned `EncodingTest.test_encoding_cyrillic_unicode` passes at
O0 and O3. The message contains no newline. The unchanged method adds a
`StreamHandler` to a named logger, logs a warning, removes and closes the
handler, reads its original `BytesIO`, and compares the bytes. At both levels
the actual comparison operand was **`e4 ee 20 f1 e2 e8 e4 e0 ed e8 ff 0a`**.

The [adapter](native_encoding.py) uses the shared suite runner to compile the
original method without editing its body. CPython runs the original module,
class and instance hooks. The native child recreates the DEBUG root, formatter
and StringIO handler observed in `BaseTest.setUp`. The public logger, handler,
codec writer, buffer, and assertion all execute in the child.

The adapter tries the shared native TestCase first. On this source its
integer equality contract rejects the bytes operands. The fallback compares
the two native byte strings, raises on inequality, and saves the actual first
operand to a binary file on success. This is recorded as **one native body
and assertion with a mixed fixture**; it does not count as native fixture
execution. The [shared assertion](https://github.com/astral-sh/imre/pull/508#issuecomment-6397086139)
and the [codec-only controls](codec-writer.md) have separate receipts.

The [control](../../../../tests/fixtures/files/logging_stream_delivery.py)
also exercises parent propagation, ordered handlers, repeated add/remove
calls, per-handler thresholds, shared stream writes, and an empty
terminator. The eleven-byte output without the newline must fail comparison
with the twelve-byte result. These controls add no suite IDs.

The unchanged `StreamHandlerTest.test_stream_setting` checks the actual
`sys.stderr` from [#606](https://github.com/astral-sh/imre/pull/606), the return
of the previous stream by identity, and the `None` return for an unchanged
stream. The shared TestCase has no native `assertIs` on this source. The
adapter verifies the diagnostic points at the original assertion before using
native `is` and `is None` on its operands. Both levels run the body and all
three assertions in Imre under the same mixed BaseTest fixture described above.

Reproduce the [Linux receipt](stream-delivery-linux.json) with the pinned interpreter:

```sh
python -I -B benchmarks/cpython_stdlib/full/logging/native_encoding.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --cpython-source /path/to/cpython --result /tmp/stream-delivery-linux.json
```

The [hosted stream run](https://github.com/astral-sh/imre/actions/runs/40217698211) verified this same receipt against sys registry revision `aaf044f`. It also compared redirected standard-stream writes with CPython and exercised import snapshots with rebound and absent values. The [full logging run](https://github.com/astral-sh/imre/actions/runs/40217698324) kept all 272 occurrences at each optimization level.

The [stream run](https://github.com/astral-sh/imre/actions/runs/40223425890) on sys registry revision `c53dc9f` reproduced the original stream, codec and sys device receipts. The [full replay](https://github.com/astral-sh/imre/actions/runs/40223428037) retained all 272 occurrences and the same execution roles; six blocked ShutdownTest methods now report updated compiler diagnostics.
