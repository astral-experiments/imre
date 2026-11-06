from pathlib import Path
from tempfile import TemporaryDirectory, TemporaryFile


with TemporaryDirectory() as directory:
    with TemporaryFile("wb+", dir=directory) as stream:
        Path(directory).rmdir()
        print(Path(directory).exists())
        print(stream.write(b"abc\ndef"))
        print(stream.tell())
        print(stream.seek(0))
        print(stream.read(1))
        print(stream.write(b"Z"))
        print(stream.readline())
        print(stream.seek(-2, 2))
        print(stream.read())
        print(stream.seek(0))
        print(stream.read())

with TemporaryFile() as stream:
    stream.write(b"abc")
    stream.seek(0)
    assert stream.read(1) == b"a"
    try:
        stream.read((1 << 63) - 1)
    except OverflowError as exc:
        assert str(exc) == "byte string is too large"
    else:
        raise AssertionError("oversized read succeeded")
    assert stream.tell() == 1
    assert stream.read() == b"bc"

stream = TemporaryFile()
stream.write(b"x" * 200000)
print(stream.tell())
print(stream.seek(131071))
print(stream.read(2))
stream.close()
stream.close()
try:
    stream.seek(0)
except ValueError as exc:
    print(str(exc))
