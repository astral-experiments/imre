import gc
import io
import os
import pickle
import tempfile
from _io import BufferedWriter as NativeWriter


class Writer(io.BufferedWriter):
    pass


class StatefulWriter(NativeWriter):
    def __getstate__(self):
        return self.raw.getvalue()

    def __setstate__(self, data):
        self.__init__(io.BytesIO(data))


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

for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
    for cls in (NativeWriter, Writer):
        with cls(io.BytesIO()) as writer:
            try:
                pickle.dumps(writer, protocol)
            except TypeError as error:
                assert str(error) == f"cannot pickle '{cls.__name__}' instances"
            else:
                raise AssertionError("pickled a writer without custom state")
    with StatefulWriter(io.BytesIO(b"state")) as writer:
        wire = pickle.dumps(writer, protocol)
    with pickle.loads(wire) as restored:
        assert type(restored) is StatefulWriter and restored.raw.getvalue() == b"state"

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
    allocate = NativeWriter.__new__
    assert allocate is object.__new__
    raises(TypeError, allocate, 7)
    raises(TypeError, allocate, int)
    for cls in (NativeWriter, Writer):
        writer = allocate(cls)
        assert type(writer) is cls and writer.raw is None
        raises(ValueError, writer.write, b"")
        raises(ValueError, writer.flush)
        raises(ValueError, writer.close)
        with io.FileIO(path, "w") as raw:
            writer.__init__(raw, 8)
            assert writer.raw is raw and writer.write(b"bare") == 4
            assert raw.tell() == 0
            writer.flush()
            assert raw.tell() == 4
            writer.close()
            assert raw.closed
        with io.FileIO(path, "r") as source:
            assert source.read() == b"bare"
    with io.FileIO(path, "w") as raw:
        writer = NativeWriter(raw, 8)
        assert writer.write(b"data") == 4 and raw.tell() == 0
        assert writer.detach() is raw and raw.tell() == 4
        assert writer.raw is None and not raw.closed
        for operation in (writer.detach, writer.flush, writer.close):
            try:
                operation()
            except ValueError as error:
                assert str(error) == "raw stream has been detached"
            else:
                raise AssertionError("used a detached writer")
        writer.__init__(raw, 8)
        assert writer.write(b"!") == 1
        writer.close()
    with io.FileIO(path, "r") as source:
        assert source.read() == b"data!"

    class HookedWriter(NativeWriter):
        def flush(self):
            if self.replacement is None:
                raise OSError("flush failed")
            self.__init__(self.replacement, 8)

    with io.FileIO(path, "w") as original:
        writer = HookedWriter(original, 8)
        writer.write(b"old")
        writer.replacement = None
        raises(OSError, writer.detach)
        assert writer.raw is original and original.tell() == 0
        with io.FileIO(path, "w") as replacement:
            writer.replacement = replacement
            assert writer.detach() is replacement and writer.raw is None
            gc.collect()
            assert not original.closed and not replacement.closed
            assert os.stat(path).st_size == 0
    with io.FileIO(path, "w") as raw:
        raises(ValueError, NativeWriter, raw, 0)
    with io.open(path, "wb") as stream:
        assert type(stream) is NativeWriter
        assert repr(stream) == f"<_io.BufferedWriter name={path!r}>"
finally:
    os.unlink(path)
