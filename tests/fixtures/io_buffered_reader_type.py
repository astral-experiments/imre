import io
import gc
import pickle
from _io import BufferedReader as NativeReader


class ReaderCase:
    tp = io.BufferedReader


class PublicReader(io.BufferedReader):
    pass


class PrivateReader(NativeReader):
    pass


assert NativeReader.__new__ is object.__new__
for cls in (NativeReader, PublicReader):
    reader = cls.__new__(cls)
    assert type(reader) is cls and reader.raw is None
    try:
        reader.read(0)
    except ValueError as error:
        assert "uninitialized" in str(error)
    else:
        raise AssertionError("read from an uninitialized reader")
    with io.FileIO(__file__, "r") as raw:
        reader.__init__(raw)
        assert reader.read(1) == b"i"
        reader.close()
        assert raw.closed


class StatefulReader(NativeReader):
    def __getstate__(self):
        return self.raw.getvalue()

    def __setstate__(self, data):
        self.__init__(io.BytesIO(data))


for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
    with StatefulReader(io.BytesIO(b"state")) as source:
        wire = pickle.dumps(source, protocol)
    with pickle.loads(wire) as restored:
        assert type(restored) is StatefulReader and restored.read() == b"state"
    for cls in (NativeReader, PublicReader):
        with cls(io.BytesIO()) as stream:
            try:
                pickle.dumps(stream, protocol)
            except TypeError as error:
                assert str(error) == f"cannot pickle '{cls.__name__}' instances"
            else:
                raise AssertionError("reader without custom state was pickled")


assert ReaderCase.tp is NativeReader
with io.open(__file__, "rb") as source:
    assert source.__iter__() is source
    assert type(source) is ReaderCase.tp
    assert isinstance(source, io.BufferedIOBase)
    assert source.read(1) == b"i"
    assert NativeReader.read(source, 5) == b"mport"

try:
    NativeReader.read(object(), 1)
except TypeError as exc:
    print(exc)
else:
    raise AssertionError("BufferedReader.read accepted an unrelated receiver")

for cls in (PublicReader, PrivateReader):
    with io.FileIO(__file__, "r") as raw, cls(raw, 4) as reader:
        assert type(reader) is cls
        assert isinstance(reader, NativeReader)
        assert reader.raw is raw
        assert NativeReader.read(reader, 1) == b"i"
        assert reader.read(5) == b"mport"

try:
    NativeReader.__init__(object())
except TypeError as exc:
    assert str(exc) == "descriptor '__init__' requires a '_io.BufferedReader' object but received a 'object'"
else:
    raise AssertionError("BufferedReader.__init__ accepted an unrelated receiver")


events = []


class ClosingFile(io.FileIO):
    def readable(self):
        events.append("readable")
        return super().readable()

    def close(self):
        events.append(("close", self.closed))
        if self.refuse:
            raise OSError("raw close")
        return super().close()


class FlushingReader(io.BufferedReader):
    def flush(self):
        events.append(("flush", self.raw.closed))
        if self.refuse:
            raise OSError("reader flush")
        return super().flush()


with io.FileIO(__file__, "r") as owner:
    raw = ClosingFile(owner.fileno(), "r", closefd=False)
    raw.refuse = True
    reader = FlushingReader(raw, 4)
    reader.refuse = True
    assert reader.raw is raw and reader.read(1) == b"i"
    assert events == ["readable"]
    try:
        reader.close()
    except OSError as exc:
        assert exc.args == ("raw close",)
        assert isinstance(exc.__context__, OSError) and exc.__context__.args == ("reader flush",)
    else:
        raise AssertionError("BufferedReader swallowed the raw close error")
    assert events[1:] == [("flush", False), ("close", False)]
    assert not reader.closed and not raw.closed
    raw.refuse = reader.refuse = False
    reader.close()
    reader.close()
    assert events[3:] == [("flush", False), ("close", False)]
    assert reader.closed and raw.closed
    owner.seek(0)
    assert owner.read(1) == b"i"


class DecliningReader(io.BufferedReader):
    def close(self):
        events.append("declined")


with io.FileIO(__file__, "r") as retained:
    events.clear()
    reader = DecliningReader(retained)
    del reader
    gc.collect()
    assert events == ["declined"] and retained.read(1) == b"i"

    with ClosingFile(__file__, "r") as replacement:
        replacement.refuse = False
        reader = io.BufferedReader(retained, 4)
        reader.peek()
        position = retained.tell()
        reader.__init__(replacement, 2)
        gc.collect()
        assert reader.raw is replacement and retained.tell() == position
        assert reader.read(1) == b"i"
        reader.close()
        assert replacement.closed and not retained.closed


class WarningFile(io.FileIO):
    def _dealloc_warn(self, source):
        events.append(("warn", source.raw is self, self.closed))
        try:
            source.peek()
        except RuntimeError:
            events.append("locked")
        raise RuntimeError("warning failed")


class ReplacingReader(io.BufferedReader):
    def flush(self):
        events.append(("flush", self.raw.closed))
        self.__init__(replacement, 2)


with WarningFile(__file__, "r") as retained, io.FileIO(__file__, "r") as replacement:
    events.clear()
    reader = ReplacingReader(retained, 2)
    reader.__del__()
    assert events == [("warn", True, False), "locked", ("flush", False)]
    assert reader.raw is replacement and replacement.closed
    assert retained.read(1) == b"i"
    reader.__del__()
    assert len(events) == 3


with io.FileIO(__file__, "r") as raw:
    def warn_from_raw(source):
        source.flush()
        events.append(source.raw is raw)
    raw._dealloc_warn = warn_from_raw
    reader = io.BufferedReader(raw)
    events.clear()
    reader.__del__()
    assert raw.closed and events == [True]


with io.open(__file__, "rb") as reader:
    assert repr(reader) == "<_io.BufferedReader name=%r>" % reader.name

raw = io.BytesIO(b"reader")
reader = PublicReader(raw)
assert repr(reader) == "<PublicReader>"
raw.name = reader
try:
    repr(reader)
except RuntimeError as exc:
    assert str(exc) == "reentrant call inside PublicReader.__repr__"
else:
    raise AssertionError("recursive reader name did not raise")
raw.name = b"rule"
assert repr(reader) == "<PublicReader name=b'rule'>"
assert reader.detach() is raw and repr(reader) == "<PublicReader>"
raw.close()


class NamedReader(io.BufferedReader):
    @property
    def name(self):
        raise OSError("name unavailable")


with NamedReader(io.BytesIO()) as reader:
    try:
        repr(reader)
    except OSError as exc:
        assert exc.args == ("name unavailable",)
    else:
        raise AssertionError("reader suppressed a name lookup error")


def check_detached_stream_lifetime(wrapper_type):
    import weakref

    raw = io.BytesIO(b"abc")
    owner = weakref.ref(raw)
    buffered = wrapper_type(raw)
    del raw
    assert buffered.raw is owner()
    assert buffered.detach() is owner()
    assert owner() is None


check_detached_stream_lifetime(io.BufferedReader)
check_detached_stream_lifetime(io.BufferedWriter)
