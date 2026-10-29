import sys

original_type = type(sys.stdout)

import _io
import io


class Wrapper(io.TextIOWrapper):
    def __init__(self, raw, **kwargs):
        super().__init__(raw, **kwargs)


def current():
    assert original_type is type(sys.stdout) is io.TextIOWrapper is _io.TextIOWrapper
    assert repr(original_type) == "<class '_io.TextIOWrapper'>"
    assert repr(Wrapper) == f"<class '{Wrapper.__module__}.{Wrapper.__qualname__}'>"
    assert isinstance(sys.stderr, io.TextIOBase) and isinstance(sys.stdout, (bytes, io.TextIOWrapper))
    assert sys.getdefaultencoding() is sys.getdefaultencoding()
    assert sys.getdefaultencoding() == "utf-8"
    raw = io.BytesIO()
    wrapper = Wrapper(raw, encoding="utf-8", line_buffering=True)
    assert type(wrapper) is Wrapper and wrapper.buffer is raw and not wrapper.isatty()
    assert wrapper.encoding is wrapper.encoding and wrapper.errors is wrapper.errors
    assert wrapper._CHUNK_SIZE == 8192 and type(io.TextIOWrapper._CHUNK_SIZE).__name__ == "getset_descriptor"
    for method, value in ((wrapper.read, "x"), (wrapper.readline, None), (wrapper.readlines, 1.5)):
        try:
            method(value)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("invalid size accepted")
    assert wrapper.write("é") == 1 and raw.getvalue() == b""
    assert wrapper.write("東\n") == 2 and raw.getvalue() == "é東\n".encode()
    assert wrapper.seek(0) == 0 and wrapper.read(1) == "é" and wrapper.read() == "東\n"
    assert wrapper.detach() is raw and not raw.closed
    try:
        wrapper.write("x")
    except ValueError as error:
        assert error.args == ("underlying buffer has been detached",)
    else:
        raise AssertionError("detached wrapper accepted a write")

    with io.TextIOWrapper(io.BytesIO(b"a\r\n\xff"), encoding="utf-8", errors="replace") as text:
        underlying = text.buffer
        assert text.readline() == "a\n" and text.read() == "\ufffd"
    assert underlying.closed and text.closed
    with io.TextIOWrapper(io.BytesIO(b"x\r\n\xe9y"), encoding="latin-1") as text:
        text._CHUNK_SIZE = 2
        assert text.readline() == "x\n" and text.tell() == 3
        assert text.read(1) == "é" and text.tell() == 4
        assert text.seek(3) == 3 and text.read() == "éy"
    with io.TextIOWrapper(io.BytesIO(b"x\xff"), encoding="utf-8") as text:
        try:
            text.read()
        except UnicodeDecodeError as error:
            assert (error.encoding, error.object, error.start, error.end) == ("utf-8", b"x\xff", 1, 2)
        else:
            raise AssertionError("strict decoder accepted an invalid byte")

    with open("wrapped-binary-data", "wb") as binary:
        text = io.TextIOWrapper(binary, encoding="utf-8")
        assert text.writable() and not text.readable() and text.seekable()
        text.write("雪\n")
        assert text.detach() is binary and binary.tell() == 4
        binary.seek(3)
        binary.write(b"!")
    with open("wrapped-binary-data", "rb") as binary:
        text = io.TextIOWrapper(binary, encoding="utf-8")
        assert text.readable() and not text.writable() and text.seekable()
        assert text.read(1) == "雪" and text.tell() == 3
        assert text.read() == "!"
        text.close()
        assert binary.closed
    raw = io.BytesIO()
    text = io.TextIOWrapper(raw, encoding="ascii")
    text.writelines(["a\r\n", "b\n", "c"])
    text.seek(0)
    assert iter(text) is text and text.readlines(3) == ["a\n", "b\n"] and next(text) == "c"
    assert text.readlines() == []
    text.close()
    raw = io.BytesIO()
    text = io.TextIOWrapper(raw, encoding="ascii")
    text.write("kept")
    assert raw.getvalue() == b""
    assert io.IOBase.__del__(text) is None and raw.closed
    raw = io.BytesIO()
    text = io.TextIOWrapper(raw)
    assert text.detach() is raw and text.__del__() is None and not raw.closed
    class Size:
        def __init__(self, owner, replacement):
            self.owner = owner
            self.replacement = replacement

        def __index__(self):
            if self.replacement is None:
                self.owner.detach()
            else:
                self.owner.__init__(self.replacement, encoding="utf-8")
            return 2

    for name in ("read", "readline"):
        stream = io.TextIOWrapper(io.BytesIO(b"old"), encoding="utf-8")
        read = getattr(stream, name)
        assert read(Size(stream, io.BytesIO(b"new"))) == "ne"
        try:
            read(Size(stream, None))
        except ValueError as error:
            assert str(error) == "underlying buffer has been detached"
        else:
            raise AssertionError("read from a detached buffer")

    print("wrapped binary streams")


if __name__ == "__main__":
    current()
