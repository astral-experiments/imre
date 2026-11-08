import errno
import os
import posix
import tempfile
from os import pwrite


class Offset:
    def __init__(self, buf, fail=False):
        self.buf = buf
        self.fail = fail

    def __index__(self):
        try:
            self.buf.append(0)
        except BufferError:
            pass
        else:
            raise AssertionError("resized an exported bytearray")
        self.buf[0] = ord("x")
        if self.fail:
            raise ValueError("offset refused")
        return 1


def exercise():
    assert pwrite is os.pwrite is posix.pwrite
    with tempfile.TemporaryFile() as stream:
        fd = stream.fileno()
        os.write(fd, b"abcd")
        os.lseek(fd, 2, os.SEEK_SET)
        buf = bytearray(b"yz")
        assert pwrite(fd, buf, Offset(buf)) == 2
        assert os.read(fd, 1) == b"z"
        assert os.pwrite(fd, memoryview(b"!"), 3) == 1
        assert posix.pwrite(fd, b"", 8) == 0
        assert os.lseek(fd, 0, os.SEEK_CUR) == 3
        assert os.pread(fd, 8, 0) == b"axz!"
        try:
            pwrite(fd, buf, Offset(buf, True))
        except ValueError as error:
            assert str(error) == "offset refused"
        else:
            raise AssertionError("accepted an invalid offset")
        buf.append(0)
        events = []

        class Exporter:
            def __buffer__(self, flags):
                events.append(("acquire", flags))
                return memoryview(buf)[:2]

            def __release_buffer__(self, view):
                events.append(("release", view.tobytes()))

        source = Exporter()
        assert os.write(fd, source) == 2
        assert pwrite(fd, source, Offset(buf)) == 2
        try:
            pwrite(fd, source, Offset(buf, True))
        except ValueError as error:
            assert str(error) == "offset refused"
        else:
            raise AssertionError("accepted an invalid offset")
        assert events == [("acquire", 0), ("release", b"xz")] * 3
    try:
        pwrite(fd, buf, 0)
    except OSError as error:
        assert error.errno == errno.EBADF
    else:
        raise AssertionError("wrote to a closed descriptor")
    previous_length = len(buf)
    buf.append(0)
    assert len(buf) == previous_length + 1


current = exercise

if __name__ == "__main__":
    exercise()
