import io
import os


class Name:
    def __init__(self, spelling):
        self.spelling = spelling
        self.calls = 0

    def __fspath__(self):
        self.calls += 1
        return self.spelling


path = Name(b"named-update")
with open(path, "w+t", encoding="utf-8", newline="\r\n") as stream:
    buffer = stream.buffer
    raw = buffer.raw
    assert path.calls == 1
    assert stream.name is raw.name is path.spelling
    assert type(buffer).__name__ == "BufferedRandom"
    assert type(raw) is io.FileIO
    assert stream.mode == "w+t" and raw.mode == buffer.mode == "rb+"
    assert stream.fileno() == raw.fileno() == buffer.fileno()
    assert stream.readable() and stream.writable() and stream.seekable()
    assert stream.write("a\n") == 2
    assert stream.seek(0) == 0 and stream.read() == "a\r\n"
assert stream.closed and buffer.closed and raw.closed

with open("named-update", "a+") as stream:
    assert stream.tell() == 3 and stream.buffer.mode == "ab+"
    stream.seek(0)
    stream.write("z")

with open("named-update", "rt+") as stream:
    assert stream.read() == "a\nz"
    stream.seek(0)
    stream.write("b")

try:
    open(path, "x+")
except FileExistsError as exc:
    assert exc.filename is path.spelling
else:
    raise AssertionError("exclusive open replaced an existing file")

with open(path, "w+", buffering=2) as stream:
    stream.buffer.write(b"x")
    assert stream.buffer.read1(0) == b"" and os.stat(path.spelling).st_size == 0
    stream.seek(0)
    stream.write("abcdef")
    stream.seek(0)
    assert stream.buffer.read1() == b"ab"

os.unlink(path.spelling)
with open(path, "x+t") as stream:
    assert stream.buffer.mode == "xb+"
    os.unlink(path.spelling)
    stream.write("open after unlink")
    stream.seek(0)
    assert stream.readline() == "open after unlink"

try:
    open(Name(1), "w+")
except TypeError as exc:
    assert str(exc) == "expected Name.__fspath__() to return str or bytes, not int"
else:
    raise AssertionError("accepted an invalid path")

print("named text update verified")
