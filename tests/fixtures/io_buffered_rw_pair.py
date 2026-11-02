import gc
import _io
import io
import os
import sys
from _io import BufferedRWPair as NativePair

class Pair(io.BufferedRWPair):
    pass

assert io.BufferedRWPair is NativePair
assert issubclass(Pair, io.BufferedIOBase)
src = "imre-rw-in-" + str(os.getpid())
dst = "imre-rw-out-" + str(os.getpid())
with io.open(src, "wb") as source:
    source.write(b"ab\ncde")
try:
    class RejectSecondCheck(io.FileIO):
        def writable(self):
            self.calls = getattr(self, "calls", 0) + 1
            return self.calls == 1

    with io.FileIO(src, "r") as raw, RejectSecondCheck(dst, "w") as rejected:
        try:
            NativePair(raw, rejected, 2)
        except io.UnsupportedOperation as exc:
            assert str(exc) == "File or stream is not writable."
        else:
            raise AssertionError("writer initialization accepted a non-writable raw")
        assert raw.closed and not rejected.closed and rejected.calls == 2

    with io.BufferedReader(io.FileIO(src, "r"), 2) as reader:
        assert reader.peek(0) == b"ab" and reader.read(1) == b"a"
        assert io.BufferedReader.peek(reader, 99) == b"b" and reader.raw.tell() == 2
    try:
        reader.peek(0)
    except ValueError as exc:
        assert str(exc) == "peek of closed file"
    else:
        raise AssertionError("closed reader allowed peek")
    raw_in = io.FileIO(src, "r")
    raw_out = io.FileIO(dst, "w")
    with Pair(raw_in, raw_out, 4) as pair:
        gc.collect()
        assert not pair.closed and pair.readable() and pair.writable()
        assert pair.peek(1) == b"ab\nc"
        assert pair.read(1) == b"a"
        assert pair.readline() == b"b\n"
        assert pair.read1() == b"c"
        assert pair.read() == b"de"
        assert NativePair.write(pair, b"foo") == 3 and raw_out.tell() == 0
        assert pair.flush() is None and raw_out.tell() == 3
        assert pair.write(b"!") == 1
    assert pair.closed and raw_in.closed and raw_out.closed
    assert pair.close() is None
    with io.open(dst, "rb") as check:
        assert check.read() == b"foo!"
    del sys.modules["_io"]
    try:
        fresh = __import__("_io")
        new_type = fresh.BufferedRWPair
        fresh.BufferedReader = fresh.BufferedWriter = object
        with new_type(io.FileIO(src, "r"), io.FileIO(dst, "w"), 4) as reopened:
            assert type(reopened) is new_type and new_type is not NativePair
            assert new_type.read(reopened, 2) == b"ab"
            try:
                NativePair.read(reopened)
            except TypeError:
                pass
            else:
                raise AssertionError("old pair descriptor accepted a new generation")
    finally:
        sys.modules["_io"] = _io
    try:
        NativePair.read(object())
    except TypeError as exc:
        assert "BufferedRWPair" in str(exc)
    else:
        raise AssertionError("BufferedRWPair accepted an unrelated receiver")
finally:
    os.unlink(src)
    os.unlink(dst)
