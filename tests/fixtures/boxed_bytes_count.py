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
    for invalid, error in ((256, ValueError), ("a", TypeError)):
        try:
            data.count(invalid)
        except error:
            pass
        else:
            raise AssertionError(invalid)
    print("binary counts matched")
