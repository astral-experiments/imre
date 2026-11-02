import array
import errno
import os
import posix
from os import readinto


class Index:
    def __init__(self, fd):
        self.fd = fd

    def __index__(self):
        return self.fd


def exercise():
    assert readinto is os.readinto is posix.readinto
    rd, wr = os.pipe()
    try:
        os.write(wr, b"abcd")
        buf = bytearray(b"......")
        view = memoryview(buf)
        released = memoryview(buf)
        released.release()
        for invalid in (b"", view[::2], view.toreadonly(), released):
            try:
                readinto(rd, invalid)
            except TypeError:
                pass
            else:
                raise AssertionError("expected a writable contiguous buffer")
        assert readinto(Index(rd), view[1:3]) == 2
        assert buf == b".ab..."
        words = array.array("H", [0])
        assert readinto(rd, words) == 2
        assert words.tobytes() == b"cd"
        os.set_blocking(rd, False)
        try:
            readinto(rd, view)
        except BlockingIOError as exc:
            assert exc.errno in (errno.EAGAIN, errno.EWOULDBLOCK)
        else:
            raise AssertionError("empty nonblocking pipe was readable")
        os.write(wr, b"e")
        assert readinto(rd, view) == 1 and buf == b"eab..."
        view.release()
        buf.extend(b"!")
    finally:
        os.close(wr)
        try:
            assert readinto(rd, bytearray(1)) == 0
        finally:
            os.close(rd)

    try:
        readinto(-1, bytearray())
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("expected EBADF even for an empty destination")


exercise()
