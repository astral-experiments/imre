import errno
import os
import posix
import tempfile
from os import pwrite


def expect_equal(actual, expected):
    if actual != expected:
        raise AssertionError((actual, expected))


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
    if not (pwrite is os.pwrite is posix.pwrite):
        raise AssertionError("pwrite aliases differ")
    with tempfile.TemporaryFile() as stream:
        fd = stream.fileno()
        expect_equal(os.write(fd, b"abcd"), 4)
        expect_equal(os.lseek(fd, 2, os.SEEK_SET), 2)
        buf = bytearray(b"yz")
        expect_equal(pwrite(fd, buf, Offset(buf)), 2)
        expect_equal(os.read(fd, 1), b"z")
        expect_equal(os.pwrite(fd, memoryview(b"!"), 3), 1)
        expect_equal(posix.pwrite(fd, b"", 8), 0)
        expect_equal(os.lseek(fd, 0, os.SEEK_CUR), 3)
        expect_equal(os.pread(fd, 8, 0), b"axz!")
        try:
            pwrite(fd, buf, Offset(buf, True))
        except ValueError as error:
            expect_equal(str(error), "offset refused")
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
        expect_equal(os.write(fd, source), 2)
        expect_equal(pwrite(fd, source, Offset(buf)), 2)
        try:
            pwrite(fd, source, Offset(buf, True))
        except ValueError as error:
            expect_equal(str(error), "offset refused")
        else:
            raise AssertionError("accepted an invalid offset")
        expect_equal(events, [("acquire", 0), ("release", b"xz")] * 3)
    try:
        pwrite(fd, buf, 0)
    except OSError as error:
        expect_equal(error.errno, errno.EBADF)
    else:
        raise AssertionError("wrote to a closed descriptor")
    previous_length = len(buf)
    buf.append(0)
    expect_equal(len(buf), previous_length + 1)


current = exercise

if __name__ == "__main__":
    exercise()
