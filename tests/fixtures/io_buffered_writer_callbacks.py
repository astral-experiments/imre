from _io import BufferedWriter
from io import RawIOBase


class Raw(RawIOBase):
    def __init__(self):
        self.chunks = []
        self.counts = []

    def writable(self):
        return True

    def write(self, view):
        assert isinstance(view, memoryview)
        assert view.readonly and view.obj is None and memoryview(view).obj is None
        count = self.counts.pop(0) if self.counts else len(view)
        if count is not None and 0 <= count <= len(view):
            self.chunks.append(bytes(view)[:count])
        return count


raw = Raw()
writer = BufferedWriter(raw, 4)
assert writer.raw is raw
assert writer.write(bytearray(b"abc")) == 3 and not raw.chunks
raw.counts = [1, None]
try:
    writer.write(b"defgh")
except BlockingIOError as exc:
    assert exc.characters_written == 2
else:
    raise AssertionError("write did not block")
assert writer.write(b"fgh") == 3
raw.counts = [4]
try:
    writer.flush()
except OSError:
    pass
else:
    raise AssertionError("overcount accepted")
assert writer.detach() is raw
assert b"".join(raw.chunks) == b"abcdefgh"
raw.close()
