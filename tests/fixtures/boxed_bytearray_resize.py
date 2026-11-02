import sys


class Buffer(bytearray):
    pass


data = Buffer(b"abcd")
view = memoryview(data)
assert bytearray.resize(data, 4) is None
for size in (0, 3, 5):
    try:
        data.resize(size)
    except BufferError:
        assert bytes(view) == b"abcd"
    else:
        raise AssertionError("resized a bytearray with an active export")
view.release()


class Size:
    def __index__(self):
        data.clear()
        data.extend(b"x")
        return 4


assert data.resize(Size()) is None
assert data == b"x\0\0\0"
data.resize(1)
data.resize(4)
assert data == b"x\0\0\0"

for size, error in ((-1, ValueError), (1.5, TypeError),
                    (sys.maxsize + 1, OverflowError), (sys.maxsize, MemoryError)):
    try:
        data.resize(size)
    except error:
        assert data == b"x\0\0\0"
    else:
        raise AssertionError("accepted an invalid bytearray size")

try:
    bytearray.resize(b"abc", 2)
except TypeError:
    pass
else:
    raise AssertionError("resized an immutable receiver")
