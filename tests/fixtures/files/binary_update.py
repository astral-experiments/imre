import errno
import io
import os
import warnings


class Path:
    def __init__(self, name):
        self.name = name
        self.calls = 0

    def __fspath__(self):
        self.calls += 1
        return self.name


path = Path(b"update-\xff")
with open(path, "xb+") as stream:
    assert type(stream).__name__ == "BufferedRandom" and isinstance(stream, io.BufferedIOBase)
    with open(path.name, "r+") as text:
        assert type(stream) is type(text.buffer)
    assert stream.name is stream.raw.name is path.name
    assert stream.mode == "xb+" and stream.raw.closefd
    assert stream.write(b"abcdef") == 6
assert path.calls == 1 and stream.closed

try:
    try:
        stream.readinto(b"")
    except TypeError as error:
        assert str(error) == "readinto() argument must be read-write bytes-like object, not bytes"
    else:
        raise AssertionError("immutable readinto target accepted on a closed file")

    try:
        stream.flush()
    except ValueError as error:
        assert str(error) == "flush of closed file"
    else:
        raise AssertionError("flushed a closed update stream")

    with open(path, "w+b", buffering=3) as stream:
        assert stream.mode == "rb+" and os.fstat(stream.fileno()).st_size == 0
        stream.write(b"abcdef")
        stream.seek(0)
        assert stream.read(1) == b"a"
        stream.write(b"Z")
        stream.seek(0)
        assert stream.read() == b"aZcdef"

    with open(path, "r+b") as stream:
        events = []

        class Offset:
            def __init__(self, value):
                self.value = value

            def __index__(self):
                events.append(self.value)
                return self.value

        def seekable():
            events.append("seekable")
            return True

        stream.raw.seekable = seekable
        assert stream.seek(Offset(2), Offset(os.SEEK_SET)) == 2
        assert events == [os.SEEK_SET, "seekable", 2]
        events.clear()
        try:
            stream.seek(Offset(2), Offset(8))
        except ValueError as error:
            assert str(error) == "whence value 8 unsupported"
        else:
            raise AssertionError("invalid whence accepted")
        assert events == [8]

    fd = os.open(path, os.O_RDWR)
    try:
        os.lseek(fd, 2, os.SEEK_SET)
        with open(fd, "wb+", closefd=False) as stream:
            assert stream.fileno() == fd and stream.name == fd
            assert stream.tell() == 2 and not stream.raw.closefd
            assert stream.read() == b"cdef"
            stream.seek(1)
            assert stream.write(b"X") == 1
        assert os.lseek(fd, 0, os.SEEK_SET) == 0
        assert os.read(fd, 6) == b"aXcdef"
    finally:
        os.close(fd)

    owned = os.open(path, os.O_RDWR)
    with open(owned, "ab+") as stream:
        assert stream.tell() == 6 and stream.raw.closefd
    try:
        os.fstat(owned)
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("owned descriptor remains open")

    for owns in (False, True):
        fd = os.open(path, os.O_RDWR)
        stream = open(fd, "r+b", buffering=8, closefd=owns)
        stream.write(b"z")
        os.close(fd)
        try:
            raise KeyError("active")
        except KeyError as active:
            try:
                stream.close()
            except OSError as exc:
                assert exc.errno == errno.EBADF and stream.closed
                if owns:
                    assert exc.__context__.errno == errno.EBADF
                    assert exc.__context__.__context__ is active
                else:
                    assert exc.__context__ is active
            else:
                raise AssertionError("flush on a closed descriptor succeeded")

    with warnings.catch_warnings():
        warnings.simplefilter("error", RuntimeWarning)
        try:
            open(path, "wb+", buffering=1)
        except RuntimeWarning:
            pass
        else:
            raise AssertionError("missing binary line-buffering warning")
    with open(path, "rb+") as stream:
        assert stream.read() == b"aXcdef"
        stream.seek(1)
        assert stream.read(2) == b"Xc"
        assert stream.truncate() == stream.tell() == 3
        assert stream.truncate(7) == 7 and stream.read() == bytes(4)

        class Size:
            def __index__(self):
                assert os.fstat(stream.fileno()).st_size == 7
                try:
                    stream.close()
                except RuntimeError as exc:
                    assert "reentrant call" in str(exc)
                else:
                    raise AssertionError("closed during buffered truncate")
                return 2

        size = Size()
        assert stream.truncate(size) is size
        assert os.fstat(stream.fileno()).st_size == 2 and stream.tell() == 7

        calls = []
        def raw_truncate(pos):
            calls.append(pos)
            return size

        stream.raw.truncate = raw_truncate
        assert stream.truncate() is size and calls == [None]
finally:
    os.unlink(path)

print("binary update verified")
