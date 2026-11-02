import errno
import os


class Index:
    def __init__(self, value: int):
        self.value = value

    def __index__(self) -> int:
        return self.value


r, w = os.pipe()
try:
    assert os.write(Index(w), b"\xff\x00xy") == 4
    assert os.read(Index(r), Index(0)) == b""
    assert os.read(r, 2) == b"\xff\x00"
    assert os.read(r, 8) == b"xy"
    assert os.write(w, b"") == 0
    try:
        os.read(r, -1)
    except OSError as exc:
        assert exc.errno == errno.EINVAL
    else:
        raise AssertionError("negative read length accepted")
    for huge in (2**100, -(2**100)):
        try:
            os.read(r, huge)
        except OverflowError as exc:
            assert str(exc) == "Python int too large to convert to C ssize_t"
        else:
            raise AssertionError("length overflow accepted")
        try:
            os.write(huge, "invalid")
        except OverflowError as exc:
            assert str(exc) == "Python int too large to convert to C int"
        else:
            raise AssertionError("descriptor overflow accepted")
    try:
        os.write(w, "invalid")
    except TypeError as exc:
        assert str(exc) == "a bytes-like object is required, not 'str'"
    else:
        raise AssertionError("text buffer accepted")
finally:
    os.close(w)
    assert os.read(r, 10) == b""
    os.close(r)
try:
    os.read(r, 0)
except OSError as exc:
    assert exc.errno == errno.EBADF
else:
    raise AssertionError("closed descriptor was readable")
print("descriptor transfer passed")
