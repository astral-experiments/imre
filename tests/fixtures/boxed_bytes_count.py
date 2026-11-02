"""Count non-overlapping byte occurrences within Python search bounds."""

from array import array


class Needle(bytes):
    def __index__(self):
        raise AssertionError("buffer takes precedence")


def current():
    data = Needle(b"aaaaa\x00\xff")
    assert bytes.count.__text_signature__ == "($self, sub[, start[, end]], /)"
    assert data.count(b"aa") == 2
    assert bytes.count(data, Needle(b"aa"), 1, -2) == 2
    assert data.count(bytearray(b"aa")) == data.count(memoryview(b"aa")) == 2
    assert data.count(array("B", [255])) == data.count(255) == 1
    assert data.count(b"", -10**80, 10**80) == len(data) + 1
    assert data.count(b"", len(data)) == 1
    assert data.count(b"", len(data) + 1) == data.count(b"", 2, 1) == 0
    assert data.find(b"aa", 1, -2) == 1
    assert bytes.find(data, Needle(b"aa")) == 0 and data.rfind(b"aa") == 3
    assert data.find(255) == 6 and data.find(memoryview(b"x")) == -1
    assert data.find(b"", len(data)) == len(data) and data.find(b"", len(data) + 1) == -1
    assert data.index(memoryview(b"aa"), 1, -2) == 1
    assert bytes.index(data, Needle(b"aa")) == 0 and data.index(255) == 6
    assert data.index(b"", len(data)) == len(data)
    for needle, start in ((b"x", 0), (b"", len(data) + 1)):
        try:
            data.index(needle, start)
        except ValueError as exc:
            assert str(exc) == "subsection not found"
        else:
            raise AssertionError((needle, start))
    for invalid, error in ((256, ValueError), ("a", TypeError)):
        try:
            data.count(invalid)
        except error:
            pass
        else:
            raise AssertionError(invalid)
    print("binary counts matched")
