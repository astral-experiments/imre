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
    print("named binary writers checked")
