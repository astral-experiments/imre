import io
from io import RawIOBase


class ChunkReader(RawIOBase):
    def __init__(self, stream):
        self.stream = stream
        self.flushes = 0

    def readable(self):
        return True

    def flush(self):
        self.flushes += 1

    def chunk_length(self):
        return int(self.stream.readline().decode("latin1").strip(), 16)


def exercise(path):
    with open(path, "rb") as stream:
        reader = ChunkReader(stream)
        with reader as entered:
            print(entered is reader, isinstance(reader, io.IOBase), RawIOBase is io.RawIOBase)
            print(reader.closed, reader.readable(), reader.writable(), reader.seekable())
            print(reader.chunk_length(), stream.tell())
            print(stream.readline(0), stream.readline(2).decode("latin1"), stream.tell())
            print(stream.read(1), stream.readline(), stream.readline())
        reader.close()
        print(reader.closed, reader.flushes, stream.seek(0), stream.read(2))
    raw = RawIOBase()
    try:
        raw.readinto(b"")
    except NotImplementedError:
        print("abstract readinto")
    raw.close()
    try:
        raw.flush()
    except ValueError:
        print("flush on closed stream")
