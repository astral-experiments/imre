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
    for method in (sink.getvalue, sink.read):
        try:
            method()
        except ValueError as error:
            assert str(error) == "I/O operation on closed file"
        else:
            raise AssertionError("StringIO read from a closed stream")
    io.StringIO.__init__(sink, "a\r\nb", newline=None)
    assert sink.read(2) == "a\n" and sink.read() == "b"
    print("StringIO reads, writes, snapshots and closes")


main()
