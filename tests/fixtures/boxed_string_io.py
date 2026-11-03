import io
import pickle
import sys

sys.path  # Exercise the boxed native path.


class Sink(io.StringIO):
    pass


def check_pickle():
    class Slotted(io.StringIO):
        __slots__ = ("slot",)

    slotted = Slotted()
    slotted.slot = 1
    for _ in range(2):
        assert slotted.__getstate__()[3] is None
    slotted.extra = 2
    del slotted.extra
    assert slotted.__getstate__()[3] == {}
    assert slotted.slot == 1

    for cls in (io.StringIO, Sink):
        stream = cls("π🌍\nx", newline="\r\n")
        stream.seek(1)
        stream.tag = "original"
        state = stream.__getstate__()
        assert state == ("π🌍\r\nx", "\r\n", 1, {"tag": "original"})
        state[3]["tag"] = "snapshot"
        assert stream.tag == "original"
        for protocol in range(2, pickle.HIGHEST_PROTOCOL + 1):
            restored = pickle.loads(pickle.dumps(stream, protocol))
            assert type(restored) is cls and restored.tag == "original"
            assert restored.getvalue() == "π🌍\r\nx"
            assert restored.tell() == 1 and restored.read(1) == "🌍"
            restored.seek(0, 2)
            restored.write("\n")
            assert restored.getvalue() == "π🌍\r\nx\r\n"
        try:
            stream.__setstate__(("", "\n", -1, None))
        except ValueError as exc:
            assert str(exc) == "position value cannot be negative"
        else:
            raise AssertionError("StringIO restored a negative position")
        stream.close()
        try:
            pickle.dumps(stream)
        except ValueError as exc:
            assert str(exc) == "I/O operation on closed file"
        else:
            raise AssertionError("pickled a closed StringIO")


def main():
    sink = Sink(initial_value="abcdef", newline="\r\n")
    assert isinstance(sink, io.TextIOBase)
    assert type(io.StringIO()) is io.StringIO
    assert (io.StringIO.__module__, io.StringIO.__name__) == ("_io", "StringIO")
    with sink as stream:
        assert stream is sink
        assert stream.write("π\x00🌍\n") == 4
        assert stream.tell() == 5
        before = stream.getvalue()
        assert before == "π\x00🌍\r\nf"
        assert stream.write("!") == 1
        assert before == "π\x00🌍\r\nf"
        assert stream.getvalue() == "π\x00🌍\r\n!"
        try:
            stream.write(b"bytes")
        except TypeError as error:
            assert str(error) == "string argument expected, got 'bytes'"
        else:
            raise AssertionError("StringIO accepted binary data")
    assert sink.closed and sink.close() is None
    for method in (sink.getvalue, sink.read, sink.readline, sink.truncate):
        try:
            method()
        except ValueError as error:
            assert str(error) == "I/O operation on closed file"
        else:
            raise AssertionError("StringIO read from a closed stream")
    io.StringIO.__init__(sink, "a\r\nb", newline=None)
    assert sink.read(2) == "a\n" and sink.read() == "b"
    try:
        sink.seek(1, 1)
    except OSError as error:
        assert str(error) == "Can't do nonzero cur-relative seeks"
    else:
        raise AssertionError("StringIO accepted a relative character offset")

    class Reset:
        def __index__(self):
            io.StringIO.__init__(sink, "π🌍\r\nlast", newline="")
            return 1

    assert sink.seek(Reset()) == 1
    assert sink.readline(2) == "🌍\r"
    assert next(sink) == "\n"
    assert sink.seek(0, 2) == 8
    assert sink.seek(10) == 10 and sink.read() == ""
    assert sink.write("!") == 1
    assert sink.getvalue() == "π🌍\r\nlast\0\0!"
    assert sink.truncate(2) == 2 and sink.tell() == 11
    assert sink.truncate(None) == 11 and sink.getvalue() == "π🌍"
    sink.close()
    try:
        sink.seek(0)
    except ValueError as error:
        assert str(error) == "I/O operation on closed file"
    else:
        raise AssertionError("StringIO sought on a closed stream")
    assert sink.encoding is None and sink.errors is None
    for name in ("line_buffering", "newlines"):
        try:
            getattr(sink, name)
        except ValueError as error:
            assert str(error) == "I/O operation on closed file"
        else:
            raise AssertionError("read a closed StringIO property")
    io.StringIO.__init__(sink, newline=None)
    assert sink.newlines is None and not sink.line_buffering
    sink.write("a\r\nb")
    assert sink.newlines == "\r\n"
    sink.write("\n")
    history = sink.newlines
    assert history == ("\n", "\r\n")
    sink.write("\r")
    sink.truncate(0)
    assert sink.newlines == ("\r", "\n", "\r\n") and history == ("\n", "\r\n")
    for name, owner in (("encoding", "_TextIOBase"), ("errors", "_TextIOBase"),
                        ("line_buffering", "StringIO"), ("newlines", "StringIO")):
        try:
            setattr(sink, name, None)
        except AttributeError as error:
            assert str(error) == f"attribute '{name}' of '_io.{owner}' objects is not writable"
        else:
            raise AssertionError("wrote to a read-only property")
    assert io.TextIOBase().newlines is None
    check_pickle()
    print("StringIO reads, writes, snapshots and closes")


main()
