import gc
import sys
import weakref
from _io import BufferedWriter
from io import BytesIO, FileIO, RawIOBase


class ReplaceOnTell(RawIOBase):
    def __init__(self, writer, inner):
        self.writer = writer
        self.inner = inner

    def writable(self):
        return True

    def tell(self):
        self.writer.__init__(self.inner, 8)
        self.writer.write(b"x")
        return 0


path = sys.argv[1]
old = FileIO(path, "w+")
writer = BufferedWriter(old, 8)
writer.write(b"a")
writer.flush()
writer.write(b"lost")

new = BytesIO()
writer.__init__(new, 8)
gc.collect()
assert writer.raw is new and not old.closed
old.seek(0)
actual = old.read()
assert actual == (b"alost" if "--invert" in sys.argv else b"a"), actual

writer.write(b"pending")
try:
    writer.__init__(old, "invalid")
except TypeError:
    pass
else:
    raise AssertionError("accepted a non-index size")
writer.flush()
assert new.getvalue() == b"pending"
ref = weakref.ref(new)
del new
gc.collect()
assert ref() is writer.raw
try:
    writer.__init__(old, 0)
except ValueError:
    pass
else:
    raise AssertionError("accepted a zero size")
assert writer.raw is old
try:
    writer.flush()
except ValueError:
    pass
else:
    raise AssertionError("used an uninitialized writer")
gc.collect()
assert ref() is None
writer.__init__(old)
writer.write(b"b")
writer.close()
assert old.closed
with FileIO(path, "r") as result:
    assert result.read() == b"ab"

# The outer initializer resets whichever buffer its position probe leaves installed.
inner = BytesIO()
writer.__init__(ReplaceOnTell(writer, inner))
assert writer.raw is inner
writer.write(b"y")
writer.flush()
assert inner.getvalue() == b"y"
writer.close()
