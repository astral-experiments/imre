import _io
import io
import gc
import os
import pickle
from typing import get_overloads


def check_state():
    stream = io.BytesIO(b"part one")
    stream.seek(5)
    stream.tag = "saved"
    state = stream.__getstate__()
    assert state == (b"part one", 5, {"tag": "saved"})
    state[2].clear()
    restored = pickle.loads(pickle.dumps(stream))
    assert restored.tag == "saved" and restored.read() == b"one"

    attrs = restored.__dict__
    with restored.getbuffer():
        try:
            restored.__setstate__((b"", 0, None))
        except BufferError:
            assert restored.getvalue() == b"part one" and restored.tell() == 8
        else:
            raise AssertionError("restored BytesIO state with a live export")
    restored.__setstate__((bytearray(b"next"), 7, {"part": 2}, None))
    assert restored.__dict__ is attrs and attrs == {"tag": "saved", "part": 2}
    assert restored.tell() == 7
    try:
        restored.__setstate__((b"new", -1, None))
    except ValueError:
        assert restored.getvalue() == b"new" and restored.tell() == 3
    else:
        raise AssertionError("accepted a negative BytesIO position")

    class Attributes(dict):
        def __iter__(self):
            return super().__iter__()

        def keys(self):
            stream.seek(1)
            stream.write(b"AB")
            stream.close()
            return super().keys()

    stream = io.BytesIO()
    stream.__setstate__((b"contents", 0, Attributes(tag="saved")))
    assert stream.__getstate__() == (b"contents", 3, {"tag": "saved"})


def current():
    check_state()
    assert _io.BytesIO is io.BytesIO
    assert _io.StringIO is io.StringIO
    assert (io.BytesIO.__module__, io.BytesIO.__name__) == ("_io", "BytesIO")

    # A fresh stream owns its exact immutable input, even after the caller drops it.
    original = bytes(bytearray(b"a\nbc"))
    first = io.BytesIO(original)
    second = io.BytesIO(original)
    assert first.getvalue() is original and second.getvalue() is original
    del original
    gc.collect()
    assert first.readline() == b"a\n" and first.seek(-1, 2) == 3
    assert first.write(b"!") == 1
    assert first.getvalue() == b"a\nb!" and second.getvalue() == b"a\nbc"
    assert second.seek(9) == 9 and second.read(1) == b"" and second.tell() == 9
    assert second.truncate(2) == 2 and second.tell() == 9 and second.getvalue() == b"a\n"
    empty = io.BytesIO(b"")
    assert empty.read(1) == b"" and empty.tell() == 0
    assert empty.seek(3) == 3 and empty.write(b"x") == 1 and empty.getvalue() == b"\0\0\0x"
    mutable = bytearray(b"abc")
    independent = io.BytesIO(mutable)
    mutable[0] = 120
    assert independent.read() == b"abc"

    stream = io.BytesIO(b"abcd")
    snapshot = stream.getvalue()
    stream.seek(1)
    view = stream.getbuffer()
    other = stream.getbuffer()
    assert type(view) is memoryview and not view.readonly
    assert view.obj is not other.obj
    child = view[1:3]
    child[:] = b"XY"
    assert bytes(other) == b"aXYd" and stream.tell() == 1
    assert stream.read() == b"XYd" and snapshot == b"abcd"
    view.release()
    other.release()
    for operation in (lambda: stream.write(b""), lambda: stream.truncate(4), stream.close):
        try:
            operation()
        except BufferError:
            pass
        else:
            raise AssertionError("modified a BytesIO with a live derived view")
    child.release()
    assert stream.write(b"!") == 1 and stream.getvalue() == b"aXYd!"
    stream.close()

    class Stream(io.BytesIO):
        pass

    read = Stream(b"abc").read
    assert read.__module__ is None
    assert read.__name__ == "read" and read.__qualname__ == Stream.__qualname__ + ".read"
    assert get_overloads(read) == [] and read(2) == b"ab"
    assert not hasattr(io.BytesIO.read, "__module__")
    assert not hasattr(read.__self__.__init__, "__module__")

    class Lines(io.BytesIO):
        def __iter__(self):
            raise AssertionError("BytesIO.readlines used the iterator override")

        def write(self, data):
            raise AssertionError("BytesIO.writelines used the write override")

    lines = Lines(b"a\nb\nend")
    assert lines.readlines(2) == [b"a\n"] and lines.tell() == 2
    assert lines.readlines(None) == [b"b\n", b"end"]
    lines.seek(0)
    snapshot = lines.getvalue()
    try:
        lines.writelines([bytearray(b"x"), b"y", "bad"])
    except TypeError:
        pass
    else:
        raise AssertionError("writelines accepted text")
    assert lines.tell() == 2 and lines.getvalue() == b"xyb\nend"
    assert snapshot == b"a\nb\nend"
    lines.close()
    try:
        lines.readlines("bad")
    except ValueError:
        pass
    else:
        raise AssertionError("readlines accepted a closed stream")

    payload = b"\x00ab\xff\nlast"
    class Source(io.BytesIO):
        def readline(self):
            raise AssertionError("BytesIO.__next__ used the readline override")

    source = Source(payload)
    iterator = iter(source)
    assert iterator is source
    assert next(iterator) == payload[:5] and source.tell() == 5
    assert source.read() == payload[5:] and next(iterator, None) is None
    source.close()
    assert iter(source) is source
    try:
        next(source)
    except ValueError:
        pass
    else:
        raise AssertionError("advanced a closed BytesIO")
    stream = Stream(initial_bytes=payload)
    assert isinstance(stream, io.BufferedIOBase)
    assert stream.getvalue() is payload and stream.read() is payload
    assert stream.seek(-4, 2) == 5
    target = bytearray(b"??????")
    assert stream.readinto(target) == 4 and target == b"last??"
    assert stream.seek(2) == 2
    assert stream.write(bytearray(b"XY")) == 2
    assert payload == b"\x00ab\xff\nlast"
    assert stream.getvalue() == b"\x00aXY\nlast"
    assert stream.seek(-100, 1) == 0
    assert stream.readline(2) == b"\x00a" and stream.readline() == b"XY\n"

    class Offset:
        def __index__(self):
            stream.seek(1)
            return 2

    assert stream.seek(Offset(), 1) == 3
    assert stream.read1(2) == b"Y\n"
    assert stream.seek(11) == 11 and stream.write(b"") == 0
    assert stream.write(b"!") == 1
    assert stream.getvalue() == b"\x00aXY\nlast\x00\x00!"
    snapshot = stream.getvalue()
    try:
        stream.truncate("5")
    except TypeError as error:
        assert str(error) == "'str' object cannot be interpreted as an integer"
    else:
        raise AssertionError("truncate accepted text")
    assert stream.truncate(100) == 100 and stream.getvalue() is snapshot
    assert stream.truncate(5) == 5 and stream.tell() == len(snapshot)
    assert stream.getvalue() == snapshot[:5] and snapshot == b"\x00aXY\nlast\x00\x00!"
    with stream as alias:
        assert alias is stream and not alias.closed
        assert stream.flush() is None
    assert alias.closed and stream.close() is None
    try:
        stream.read()
    except ValueError as error:
        assert str(error) == "I/O operation on closed file."
    else:
        raise AssertionError("read from closed stream")

    class NeverIndex:
        def __index__(self):
            raise AssertionError("truncate called __index__ on a closed stream")

    try:
        stream.truncate(NeverIndex())
    except ValueError as error:
        assert str(error) == "I/O operation on closed file."
    else:
        raise AssertionError("truncated a closed stream")

    source = bytearray(b"mutable")
    try:
        io.BytesIO.__init__(stream, source)
    except ValueError as error:
        assert str(error) == "I/O operation on closed file."
    else:
        raise AssertionError("reopened a closed stream from a mutable buffer")
    io.BytesIO.__init__(stream, b"reopened")
    io.BytesIO.__init__(stream, source)
    source[0] = 88
    assert stream.read() == b"mutable"
    try:
        stream.__init__("text")
    except TypeError as error:
        assert str(error) == "a bytes-like object is required, not 'str'"
    else:
        raise AssertionError("accepted a text buffer")
    assert stream.tell() == 0 and stream.getvalue() == b""
    try:
        stream.seek(0, 7)
    except ValueError as error:
        assert str(error) == "invalid whence (7, should be 0, 1 or 2)"
    else:
        raise AssertionError("accepted invalid whence")
    class Reader(io.BufferedIOBase):
        def read(self, size):
            return b"abc"[:size]

        def read1(self, size):
            return self.read(min(1, size))

    reader = Reader()
    dest = bytearray(b"????")
    assert reader.readinto(dest) == 3 and dest == b"abc?"
    dest[:] = b"????"
    assert reader.readinto1(dest) == 1 and dest == b"a???"
    assert reader.readinto(bytearray()) == 0
    assert io.BytesIO(b"xy").readinto1(dest) == 2 and dest == b"xy??"

    class Resizes(io.BufferedIOBase):
        def read(self, size):
            dest.clear()

    try:
        Resizes().readinto(dest)
    except BufferError:
        assert dest == b"xy??"
    else:
        raise AssertionError("reader resized the exported bytearray")
    dest.clear()
    assert io.StringIO("π").getvalue() == "π"

    # Retry an interrupted raw callback with the same writable destination.
    class Interrupted(io.RawIOBase):
        def readable(self):
            return True

        def readinto(self, dest):
            import errno
            self.dest = dest
            self.readinto = self.finish
            raise InterruptedError(errno.EINTR, "interrupted")

        def finish(self, dest):
            assert dest is self.dest
            dest[0] = 88
            return 1


    with io.BufferedReader(Interrupted()) as stream:
        assert stream.read1(1) == b"X"

    class Raw(io.RawIOBase):
        def readable(self):
            return True

        def readinto(self, target):
            assert type(target) is memoryview and target.obj is None
            self.view = target
            target[:4] = b"abcd"
            return 4

    class Reader(io.BufferedReader):
        def read(self, size=-1):
            return b"seen:" + super().read(size)

    raw = Raw()
    reader = Reader(raw, buffer_size=4)
    assert reader.raw is raw and reader.peek() == b"abcd"
    raw.view[1:3] = b"xy"
    assert reader.read(4) == b"seen:axyd"
    reader.close()
    assert reader.closed and raw.closed

    class DelegatingRaw(Raw):
        def readinto(self, target):
            target[:] = b"[..]"
            assert self.source.readinto(target[1:3]) == 2
            return 4

    # Delegate reads must only write the middle of the raw callback's target.
    read_fd, write_fd = os.pipe()
    try:
        assert os.write(write_fd, b"fd") == 2
    finally:
        os.close(write_fd)
    with io.FileIO(read_fd, "rb") as file_source, io.BytesIO(b"io") as memory_source:
        for source, expected in ((file_source, b"fd]"), (memory_source, b"io]")):
            raw = DelegatingRaw()
            raw.source = source
            with io.BufferedReader(raw, 4) as reader:
                assert reader.read(1) == b"["
                assert reader.read1(3) == expected

    first = io.BytesIO(b"first")
    second = io.BytesIO(b"second")
    reader = io.BufferedReader(first, 4)
    assert reader.read(2) == b"fi"
    try:
        reader.__init__(second, 0)
    except ValueError:
        assert reader.raw is second and not first.closed
    else:
        raise AssertionError("invalid buffer size accepted")
    try:
        reader.readable()
    except ValueError:
        pass
    else:
        raise AssertionError("failed initialization left a usable reader")
    reader.__init__(second, 4)
    assert reader.read(2) == b"se"

    calls = []
    class SeekOffset:
        def __index__(self):
            calls.append("offset")
            assert seek_reader.read(1) == b"a"
            return 1

    with io.BufferedReader(io.BytesIO(b"abc"), 2) as seek_reader:
        assert seek_reader.seek(SeekOffset(), 1) == 2
        assert seek_reader.read() == b"c"
    for whence, message in ((0, "seek of closed file"), (9, "whence value 9 unsupported")):
        try:
            seek_reader.seek(SeekOffset(), whence)
        except ValueError as error:
            assert str(error) == message
        else:
            raise AssertionError("closed reader accepted seek")
    with io.BufferedReader(Raw()) as unseekable:
        try:
            unseekable.seek(SeekOffset())
        except io.UnsupportedOperation:
            pass
        else:
            raise AssertionError("nonseekable reader accepted seek")
    assert calls == ["offset"]

    class Uninitialized(io.BufferedReader):
        def __init__(self):
            pass

    try:
        Uninitialized().detach()
    except ValueError as error:
        assert "uninitialized" in str(error)
    else:
        raise AssertionError("uninitialized reader detached")

    assert reader.detach() is second
    assert reader.raw is None and not second.closed
    # Releasing the wrapper never closes or rewinds its raw stream.
    assert second.read() == b"nd"
    for operation in (reader.detach, reader.read, reader.flush):
        try:
            operation()
        except ValueError as error:
            assert "detached" in str(error)
        else:
            raise AssertionError("operation on detached reader")

    # The destination is exported before a raw property can run Python.
    dest = bytearray(2)

    class ResizingRaw(io.BytesIO):
        @property
        def closed(self):
            if self.check_export:
                try:
                    dest.append(0)
                except BufferError:
                    pass
                else:
                    raise AssertionError("resized the destination during readinto")
            return super().closed

    raw = ResizingRaw(b"xy")
    raw.check_export = True
    reader = io.BufferedReader(raw, 2)
    assert reader.readinto1(dest) == 2
    assert dest == b"xy"
    raw.check_export = False
    reader.close()
    print("BytesIO retains native stream position and immutable snapshots")
