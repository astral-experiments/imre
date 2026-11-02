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


def warn(source):
    events.append(source)
    if source is writer:
        try:
            source.flush()
        except RuntimeError:
            events.append("locked")
    raise OSError("ignored warning hook")


if sys.argv[2] == "file-finalize":
    writer = BufferedWriter(FileIO(sys.argv[1], "wb"), 8)
else:
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
elif sys.argv[2] in ("file-finalize", "callback-finalize"):
    events = []
    raw = writer.raw
    raw._dealloc_warn = warn
    assert writer._dealloc_warn(None) is None
    writer.__del__()
    writer.__del__()
    assert events == [None, writer, "locked"] and raw.closed
