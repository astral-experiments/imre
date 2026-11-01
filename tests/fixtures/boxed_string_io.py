import io
import sys

sys.path  # Exercise the boxed native path.


class Sink(io.StringIO):
    pass


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
    print("StringIO reads, writes, snapshots and closes")


main()
