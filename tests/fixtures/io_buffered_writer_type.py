import gc
import io
import os
import tempfile
from _io import BufferedWriter as NativeWriter


class Writer(io.BufferedWriter):
    pass


class KeepsRawOpen(io.BufferedWriter):
    def close(self):
        pass


def raises(expected, operation, *args):
    try:
        operation(*args)
    except expected:
        return
    raise AssertionError(f"{operation} did not raise {expected}")


assert io.BufferedWriter is NativeWriter
assert issubclass(Writer, io.BufferedIOBase)
assert issubclass(Writer, io.IOBase)
assert not issubclass(Writer, io.RawIOBase)
assert not issubclass(Writer, io.TextIOBase)
raises(TypeError, NativeWriter.write, object(), b"x")
raises(TypeError, NativeWriter.__init__, object())

fd, path = tempfile.mkstemp()
try:
    os.close(fd)
    with io.FileIO(path, "w") as raw:
        writer = KeepsRawOpen(raw, 8)
        writer.write(b"abc")
        del writer
        gc.collect()
        assert not raw.closed
        assert raw.tell() == 0
    for cls in (NativeWriter, Writer):
        with io.FileIO(path, "w") as raw:
            writer = cls(raw, 8)
            assert type(writer) is cls
            assert isinstance(writer, NativeWriter)
            assert isinstance(writer, io.BufferedIOBase)
            assert not isinstance(raw, NativeWriter)
            assert writer.raw is raw
            label = "_io.BufferedWriter" if cls is NativeWriter else "Writer"
            assert repr(writer) == f"<{label} name={path!r}>"
            raw.name = writer
            try:
                repr(writer)
            except RuntimeError as exc:
                assert str(exc) == f"reentrant call inside {label}.__repr__"
            else:
                raise AssertionError("recursive writer name did not raise")
            raw.name = b"out"
            assert repr(writer) == f"<{label} name=b'out'>"
            raises(TypeError, writer.write, "text")
            view = memoryview(b"x")
            view.release()
            raises(ValueError, writer.write, view)
            assert NativeWriter.write(writer, b"abc") == 3
            assert raw.tell() == 0
            assert writer.flush() is None
            assert raw.tell() == 3
            assert writer.write(b"d") == 1
            assert writer.close() is None
            assert raw.closed and writer.closed
            assert writer.close() is None
            raises(ValueError, writer.write, b"e")
            raises(ValueError, writer.flush)
        with io.FileIO(path, "r") as source:
            assert source.read() == b"abcd"
            raises(io.UnsupportedOperation, NativeWriter, source)
    with io.FileIO(path, "w") as raw:
        raises(ValueError, NativeWriter, raw, 0)
    with io.open(path, "wb") as stream:
        assert type(stream) is NativeWriter
        assert repr(stream) == f"<_io.BufferedWriter name={path!r}>"
finally:
    os.unlink(path)
