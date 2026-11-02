import io
import os
from tempfile import NamedTemporaryFile


def current():
    for mode in ("wb", "bw"):
        with NamedTemporaryFile(mode, dir=".", delete=False) as writer:
            path = writer.name
            assert writer.file.name == path and writer.mode == "wb"
            assert writer.writable() and not writer.readable()
            fd = writer.fileno()
            assert not os.get_inheritable(fd)
            try:
                writer.read(1)
            except io.UnsupportedOperation:
                pass
            else:
                raise AssertionError("write-only file allowed a read")
            assert writer.write(b"first\0\xff") == 7
            assert os.lseek(fd, 0, os.SEEK_CUR) == 0
            writer.flush()
            os.lseek(fd, 0, os.SEEK_SET)
            assert os.read(fd, 7) == b"first\0\xff"
        try:
            assert writer.closed
            with open(path, "rb") as reader:
                assert reader.read() == b"first\0\xff"
        finally:
            os.unlink(path)
    with NamedTemporaryFile("wb", dir=".") as writer:
        path = writer.name
    assert not os.path.exists(path)

    writer = NamedTemporaryFile("wb", dir=".", delete_on_close=False)
    path = writer.name
    writer.close()
    assert writer.closed and os.path.exists(path)
    del writer
    assert not os.path.exists(path)

    writer = NamedTemporaryFile("wb", dir=".")
    path, write = writer.name, writer.write
    del writer
    assert write(b"ok") == 2 and os.path.exists(path)
    del write
    assert not os.path.exists(path)

    for mode in ("w", "wt", "tw"):
        with NamedTemporaryFile(mode, dir=".", encoding="latin-1", newline="\r\n") as writer:
            path = writer.name
            assert writer.mode == mode and writer.file.name == path
            assert writer.writable() and not writer.readable()
            assert writer.write("é\n") == 2
            writer.flush()
            with open(path, "rb") as reader:
                assert reader.read() == b"\xe9\r\n"
        assert not os.path.exists(path)

    try:
        NamedTemporaryFile("w", dir="missing-directory", errors=1)
    except TypeError as error:
        assert str(error) == "open() argument 'errors' must be str or None, not int"
    else:
        raise AssertionError("invalid error handler type accepted")

    before = set(os.listdir("."))
    try:
        NamedTemporaryFile("w", dir=".", encoding="bad-encoding", delete=False)
    except LookupError:
        pass
    else:
        raise AssertionError("unknown encoding accepted")
    assert set(os.listdir(".")) == before
    print("named writers checked")
