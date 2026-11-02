import sys

original_type = type(sys.stdout)

import _io
import _locale
import io
import locale
import os
import weakref


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
    assert locale.getencoding is _locale.getencoding
    with io.TextIOWrapper(io.BytesIO(), encoding="locale") as text:
        assert text.encoding == locale.getencoding()
    for args, kwargs in (((1,), {}), ((), {"encoding": "ascii"})):
        try:
            locale.getencoding(*args, **kwargs)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("getencoding accepted arguments")
    bad = "x" + chr(0xd800) + chr(0xdfff)
    for options in ({"encoding": bad}, {"errors": bad}, {"errors": 42, "newline": bad}):
        try:
            io.TextIOWrapper(io.BytesIO(), **options)
        except UnicodeEncodeError as error:
            assert (error.encoding, error.start, error.end) == ("utf-8", 1, 3)
            assert error.object is bad and error.reason == "surrogates not allowed"
        else:
            raise AssertionError("accepted surrogates in text options")
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

    raw = io.BytesIO()
    with io.TextIOWrapper(raw, encoding="latin-1", errors="replace", newline="\n") as text:
        text.write("é\n")
        assert raw.getvalue() == b""
        text.reconfigure(encoding="utf-8-sig", newline="\r\n", write_through=True)
        assert raw.getvalue() == b"\xe9\n" and text.errors == "strict"
        text.write("é\n")
        assert raw.getvalue() == b"\xe9\n\xc3\xa9\r\n"

    class Encoding(str):
        pass

    with io.TextIOWrapper(io.BytesIO()) as text:
        observed = []
        encoding = Encoding("ascii")
        ref = weakref.ref(encoding, lambda _: observed.append((text.encoding, text.errors)))
        text.reconfigure(encoding=encoding, errors="replace")
        del encoding
        assert ref() is text.encoding
        text.reconfigure(encoding="latin-1", errors="ignore")
        assert ref() is None and observed == [("latin-1", "replace")]

    with io.TextIOWrapper(io.BytesIO(b"a\rb"), encoding="ascii") as text:
        assert text.read(1) == "a"
        text.reconfigure(line_buffering=True)
        try:
            text.reconfigure(newline=None)
        except io.UnsupportedOperation:
            pass
        else:
            raise AssertionError("newline changed while decoded text was buffered")
        assert text.read() == "\nb"

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

    with open("wrapped-binary-data") as reader:
        fd = reader.fileno()
        assert io.TextIOWrapper.fileno(reader) == fd
        assert os.read(fd, 3) == "雪".encode() and reader.read() == "!"
    try:
        reader.fileno()
    except ValueError as error:
        assert str(error) == "I/O operation on closed file"
    else:
        raise AssertionError("closed reader returned a descriptor")

    class Buffer(io.BytesIO):
        @property
        def closed(self):
            return True

        def fileno(self):
            return self

    buffer = Buffer()
    wrapper = io.TextIOWrapper(buffer)
    assert wrapper.fileno() is buffer
    wrapper.close()

    raw = io.BytesIO()
    text = io.TextIOWrapper(raw, encoding="ascii")
    text.writelines(["a\r\n", "b\n", "c"])
    text.seek(0)
    assert iter(text) is text and text.readlines(3) == ["a\n", "b\n"] and next(text) == "c"
    assert text.readlines() == []
    text.close()
    with io.TextIOWrapper(io.BytesIO(b"a\nb\nc\n"), encoding="ascii") as text:
        assert iter(text) is text and text.tell() == 0
        assert next(text) == "a\n" and text.readline() == "b\n"
        try:
            text.seek(0, 1)
        except OSError as error:
            assert str(error) == "telling position disabled by next() call"
        else:
            raise AssertionError("relative seek accepted during iteration")
        text.flush()
        assert text.tell() == 6 and next(text) == "c\n"
        assert text.readline() == ""
        try:
            text.tell()
        except OSError as error:
            assert str(error) == "telling position disabled by next() call"
        else:
            raise AssertionError("readline at EOF restored telling")
        assert next(text, None) is None and text.tell() == 6
        text.seek(0)
        assert io.IOBase.__next__(text) == "a\n" and text.tell() == 2
        assert next(text) == "b\n"
        text.flush()
        assert text.tell() == 4  # The earlier readline retained a decoder position.
        text.seek(0)
        text._CHUNK_SIZE = 4
        assert text.readline() == "a\n" and next(text) == "b\n" and next(text) == "c\n"
        text.flush()
        assert text.buffer.tell() == 6 and text.tell() == 4
        text.seek(0)
        assert next(text) == "a\n"
        text.flush()
        assert isinstance(text.tell(), int)

    with io.TextIOWrapper(io.BytesIO("é\nXabc\n".encode()), encoding="utf-8") as text:
        text._CHUNK_SIZE = 4
        assert text.readline() == "é\n" and next(text) == "Xabc\n"
        text.flush()
        try:
            text.tell()
        except OSError as error:
            assert str(error) == "can't reconstruct logical file position"
        else:
            raise AssertionError("position exceeded the saved decoder input")

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
