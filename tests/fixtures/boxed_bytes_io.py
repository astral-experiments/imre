import _io
import io


def current():
    assert _io.BytesIO is io.BytesIO
    assert _io.StringIO is io.StringIO
    assert (io.BytesIO.__module__, io.BytesIO.__name__) == ("_io", "BytesIO")

    class Stream(io.BytesIO):
        pass

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
    print("BytesIO retains native stream position and immutable snapshots")
