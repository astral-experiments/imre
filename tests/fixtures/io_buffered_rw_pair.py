import gc
import _io
import io
import os
import sys
import weakref
from _io import BufferedRWPair as NativePair

class Pair(io.BufferedRWPair):
    pass

def check_unsupported_truncate(stream):
    try:
        stream.truncate(0)
    except io.UnsupportedOperation as exc:
        assert exc.args == ("truncate",)
    else:
        raise AssertionError("BufferedRWPair accepted truncate")

assert io.BufferedRWPair is NativePair
assert issubclass(Pair, io.BufferedIOBase)
assert NativePair.truncate is _io._IOBase.truncate
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

    events = []
    def record(event):
        events.append((event.exc_type, event.exc_value.args, event.err_msg,
                       event.object, event.exc_traceback is not None))

    class Invalidated(RejectSecondCheck):
        def writable(self):
            allowed = super().writable()
            if not allowed:
                os.close(raw.fileno())
            return allowed

    saved_hook = sys.unraisablehook
    sys.unraisablehook = record
    try:
        with io.FileIO(src, "r") as raw, Invalidated(dst, "w") as rejected:
            try:
                NativePair(raw, rejected, 2)
            except io.UnsupportedOperation as exc:
                assert str(exc) == "File or stream is not writable." and exc.__context__ is None
            else:
                raise AssertionError("failed writer initialization returned a pair")
            assert raw.closed and not rejected.closed and len(events) == 1
            kind, args, message, obj, traceback = events[0]
            assert kind is OSError and args[0] == 9  # EBADF from closing the reader.
            assert message.startswith("Exception ignored while finalizing file <_io.BufferedReader")
            assert obj is None and traceback
        gc.collect()
        assert len(events) == 1

        class BadClose(io.IOBase):
            def close(self):
                raise RuntimeError("close failed")
            def __repr__(self):
                return "bad stream"
        stream = BadClose()
        io.IOBase.__del__(stream)
        io.IOBase.close(stream)
        assert events[1:] == [(RuntimeError, ("close failed",),
                               "Exception ignored while finalizing file bad stream", None, True)]
    finally:
        sys.unraisablehook = saved_hook

    class WithNew(NativePair):
        def __new__(cls, *args, **kwargs):
            return super().__new__(cls)
    with WithNew(io.FileIO(src, "r"), io.FileIO(dst, "w"), tag=True) as pair:
        assert pair.read(1) == b"a"

    class Alias(NativePair):
        __new__ = object.__new__
    class Wrapped(NativePair):
        __new__ = staticmethod(object.__new__)
    with io.FileIO(src, "r") as raw, io.FileIO(dst, "w") as writer:
        try:
            NativePair.__init__(object.__new__(Alias), raw, writer, tag=True)
        except TypeError as exc:
            assert str(exc) == "BufferedRWPair() takes no keyword arguments"
        else:
            raise AssertionError("alias of the base allocator accepted keywords")
        wrapped = object.__new__(Wrapped)
        NativePair.__init__(wrapped, raw, writer, tag=True)
        wrapped.close()

    with io.BufferedReader(io.FileIO(src, "r"), 2) as reader:
        assert reader.peek(0) == b"ab" and reader.read(1) == b"a"
        assert io.BufferedReader.peek(reader, 99) == b"b" and reader.raw.tell() == 2
    try:
        reader.peek(0)
    except ValueError as exc:
        assert str(exc) == "peek of closed file"
    else:
        raise AssertionError("closed reader allowed peek")
    memory = io.BytesIO()
    with io.FileIO(src, "r") as raw, NativePair(raw, memory, 4) as mixed:
        assert mixed.peek() == b"ab\nc" and mixed.read(1) == b"a"
        assert mixed.write(b"xyz") == 3 and memory.getvalue() == b""
        assert mixed.flush() is None and memory.getvalue() == b"xyz"
        assert mixed.write(b"!") == 1
    assert raw.closed and memory.closed
    raw_in = io.FileIO(src, "r")
    raw_out = io.FileIO(dst, "w")
    with Pair(raw_in, raw_out, 4) as pair:
        gc.collect()
        assert not pair.closed and pair.readable() and pair.writable()
        check_unsupported_truncate(pair)
        assert pair.peek(1) == b"ab\nc"
        assert pair.read(1) == b"a"
        assert pair.readline() == b"b\n"
        assert pair.read1() == b"c"
        assert pair.read() == b"de"
        assert NativePair.write(pair, b"foo") == 3 and raw_out.tell() == 0
        assert pair.flush() is None and raw_out.tell() == 3
        assert pair.write(b"!") == 1
    assert pair.closed and raw_in.closed and raw_out.closed
    check_unsupported_truncate(pair)
    assert pair.close() is None
    with io.open(dst, "rb") as check:
        assert check.read() == b"foo!"
    class ObserveReader(io.FileIO):
        def close(self):
            events.append(("writer", reader.closed))
            return super().close()

    for kind, cyclic, reader_closed in ((NativePair, False, True), (Pair, False, False),
                                        (NativePair, True, False)):
        reader = io.FileIO(src, "r")
        writer = ObserveReader(dst, "w")
        pair = kind(reader, writer, 8)
        events = []
        assert pair.write(b"x") == 1
        def gone(ref):
            assert ref() is None
            events.append(("weak", reader.closed, writer.closed))
        ref = weakref.ref(pair, gone)
        if cyclic:
            pair.loop = pair
        retained_method = pair.write
        del pair
        assert not events and ref() is not None
        del retained_method
        if cyclic:
            assert not events
        gc.collect()
        # A Python subtype runs IOBase's finalizer before ordinary weakref clearing.
        if kind is Pair:
            assert events == [("writer", False), ("weak", True, True)]
        else:
            assert events == [("weak", False, False), ("writer", reader_closed)]
        assert reader.closed and writer.closed
        with io.FileIO(dst, "r") as check:
            assert check.read() == b"x"
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
