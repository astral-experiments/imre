import io
from _io import BufferedReader as NativeReader


class ReaderCase:
    tp = io.BufferedReader


class PublicReader(io.BufferedReader):
    pass


class PrivateReader(NativeReader):
    pass


assert ReaderCase.tp is NativeReader
with io.open(__file__, "rb") as source:
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
