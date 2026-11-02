import sys
from _io import BufferedWriter, FileIO


class Raw:
    def __init__(self, path):
        self.file = FileIO(path, "wb")
        self.temporarily_closed = False

    def writable(self):
        return True

    def tell(self):
        return self.file.tell()

    @property
    def closed(self):
        return self.temporarily_closed or self.file.closed

    def write(self, view):
        return self.file.write(view)

    def close(self):
        return self.file.close()


writer = BufferedWriter(Raw(sys.argv[1]), 8)
assert writer.write(b"abc") == 3
if sys.argv[2] == "close":
    writer.close()
elif sys.argv[2] == "temporary-close":
    raw = writer.raw
    raw.temporarily_closed = True
    writer.close()
    raw.temporarily_closed = False
    writer.flush()
    writer.close()
