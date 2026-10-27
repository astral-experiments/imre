import _io
import io


def current():
    assert _io.BytesIO is io.BytesIO
    assert _io.StringIO is io.StringIO
    assert (io.BytesIO.__module__, io.BytesIO.__name__) == ("_io", "BytesIO")

    class Stream(io.BytesIO):
        pass

    payload = b"\x00ab\xff\nlast"
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
    assert io.StringIO("π").getvalue() == "π"
    print("BytesIO retains native stream position and immutable snapshots")
