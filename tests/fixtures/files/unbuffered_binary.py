import io
from _io import FileIO

assert FileIO is io.FileIO
path = "exclusive.bin"
with open(path, "xb", 0) as writer:
    assert type(writer) is io.FileIO
    assert writer.mode == "xb" and writer.writable() and not writer.readable()
    assert writer.write(b"A\x00\xff") == 3
    with open(path, "rb") as reader:
        assert reader.read() == b"A\x00\xff"

    try:
        open(path, "xb", 0)
    except FileExistsError as exc:
        assert exc.filename == path
    else:
        raise AssertionError("exclusive open overwrote an existing file")
assert writer.closed

with io.FileIO(path, "r") as reader:
    fd = reader.fileno()
    with FileIO(fd, closefd=False) as borrowed:
        assert borrowed.fileno() == fd
        assert repr(borrowed) == f"<_io.FileIO name={fd!r} mode='rb' closefd=False>"
        assert borrowed.read(2) == b"A\x00"
    assert reader.tell() == 2
    assert reader.seek(-1, 2) == 2
    assert reader.read() == b"\xff"
    assert reader.read() == b""
print("unbuffered binary file verified")
