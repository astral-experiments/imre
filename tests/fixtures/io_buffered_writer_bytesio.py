import gc
import io
import sys

raw = io.BytesIO()
writer = io.BufferedWriter(raw, 4)
assert writer.raw is raw
assert writer.write(b"abcd") == 4 and raw.getvalue() == b"abcd"
assert writer.write(b"ef") == 2 and writer.write(b"gh") == 2
assert raw.getvalue() == b"abcd" and writer.tell() == 8
for offset, whence, error in [("bad", 0, TypeError), (0, 99, ValueError)]:
    try:
        writer.seek(offset, whence)
    except error:
        assert raw.getvalue() == b"abcd"
    else:
        raise AssertionError("invalid seek succeeded")
assert writer.detach() is raw
assert not raw.closed and raw.getvalue() == b"abcdefgh"

text = io.TextIOWrapper(io.BufferedWriter(raw), encoding="utf-8", newline="")
assert text.write("é\n") == 2
assert raw.getvalue() == b"abcdefgh"
text.flush()
expected = b"abcdefgh\xc3\xa9\n"
if "--invert" in sys.argv:
    expected = b"abcdefghWRONG\n"
assert raw.getvalue() == expected, (raw.getvalue(), expected)
text.close()
assert raw.closed

raw = io.BytesIO()
writer = io.BufferedWriter(raw)
writer.write(b"final")
del writer
gc.collect()
assert raw.closed
print("buffered BytesIO OK")
