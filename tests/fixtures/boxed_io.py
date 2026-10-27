import io
import sys
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


class BufferedInput(RawIOBase):
    def __init__(self, stream):
        self.stream = stream

    def readinto(self, buffer):
        return self.stream.readinto(buffer)


class ClosingOffset:
    def __init__(self, reader):
        self.reader = reader

    def __index__(self):
        self.reader.close()
        return 0


class ReadCount(RawIOBase):
    def __init__(self, count):
        self.count = count

    def readinto(self, buffer):
        for i in range(1600):
            _ = [i]
        return self.count


class ReadSize:
    def __init__(self, stream):
        self.stream = stream

    def __index__(self):
        print("size consumed", self.stream.read(1))
        return 1


class ClosingWriterSize:
    def __init__(self, writer):
        self.writer = writer

    def __index__(self):
        self.writer.close()
        return 0


def exercise(path):
    with open(path, "rb") as reader:
        reader.read(1)
        try:
            reader.read(sys.maxsize)
        except OverflowError as error:
            print(error.args, reader.tell(), reader.read(1))
    with open(path, "ab") as writer:
        print(writer.readline(ClosingWriterSize(writer)), writer.closed)
    with open(path, "ab") as writer:
        try:
            writer.read(ClosingWriterSize(writer))
        except io.UnsupportedOperation as error:
            print(error.args, writer.closed)
    with open(path, "rb") as stream:
        print(stream.read(ReadSize(stream)), stream.tell())
        stream.seek(0)
        print(stream.readline(ReadSize(stream)), stream.tell())
    from io import UnsupportedOperation

    target_path = path + ".output"
    with open(target_path, "wb") as output:
        alias = output
        print(output.write(b"\x00\xff"), alias.closed)
        with open(target_path, "rb") as reader:
            print(reader.read())
        alias.flush()
        with open(target_path, "rb") as reader:
            print(reader.read())
    with open(target_path, "ab") as output:
        print(output.write(bytearray(b"x")))
    with open(target_path, "rb") as reader:
        print(reader.read())
    try:
        open(target_path, "xb")
    except FileExistsError:
        print("exclusive create")
    try:
        alias.read()
    except UnsupportedOperation as error:
        print(alias.closed, isinstance(error, OSError), isinstance(error, ValueError))
    for operation, argument in ((alias.read, 1 << 40), (alias.readline, 1 << 100),
                                (alias.readinto, b""), (alias.read, -2)):
        try:
            operation(argument)
        except (OverflowError, TypeError, OSError) as error:
            print(type(error).__name__)
    try:
        alias.write(b"x")
    except ValueError:
        print("closed writer")

    for invalid in ("./missing-binary-input/file", path + "/child", "."):
        try:
            open(invalid, "rb")
        except OSError as error:
            print(type(error).__name__, error.errno, error.strerror, error.filename, error.args)
    try:
        open("nul\x00path", "rb")
    except ValueError as error:
        print(str(error))
    with open(path, "rb") as stream:
        reader = ChunkReader(stream)
        with reader as entered:
            print(entered is reader, isinstance(reader, io.IOBase), RawIOBase is io.RawIOBase)
            print(reader.closed, reader.readable(), reader.writable(), reader.seekable())
            print(callable(io.RawIOBase.readinto), io.IOBase.readable(reader))
            try:
                reader.closed = True
            except AttributeError:
                print("closed is read-only")
            print(reader.chunk_length(), stream.tell())
            print(stream.readline(0), stream.readline(2).decode("latin1"), stream.tell())
            print(stream.read(1), stream.readline(), stream.readline())
        reader.close()
        print(reader.closed, reader.flushes, stream.seek(0), stream.read(2))
        try:
            stream.read(-2)
        except ValueError:
            print(stream.tell(), stream.readline(-2))
    try:
        stream.read(1 << 100)
    except OverflowError:
        print("closed read validates the size")
    for operation in (
        lambda: stream.read(0),
        lambda: stream.readline(0),
        lambda: next(stream, b"fallback"),
        lambda: stream.readinto(bytearray(1)),
        lambda: stream.seek(0),
        lambda: stream.__enter__(),
        lambda: stream.flush(),
        lambda: stream.tell(),
    ):
        try:
            operation()
        except ValueError as error:
            print(error.args)
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
    print(ReadCount(None).read(2) is None, ReadCount(None).readall() is None)
    for count in (-1, 3):
        try:
            ReadCount(count).read(2)
        except ValueError as error:
            print(error)

    with open(path, "rb") as stream:
        source = BufferedInput(stream)
        print(source.read(2), source.read(), source.read(1))
        stream.seek(0)
        target = bytearray(b"......")
        alias = target
        short = bytearray(2)
        target[:stream.readinto(short)] = short
        print(bytes(alias), stream.readinto(target), bytes(alias))
        try:
            stream.readinto(b"")
        except TypeError:
            print("writable buffer required")
        target[::-2] = [65, 66, 67]
        target[2:1] = b"x"
        target[-1] = 42
        target.extend(target)
        print(bytes(target), target == bytearray(target))
        try:
            target[::2] = b"x"
        except ValueError:
            print("extended slice length mismatch")
        target[::-2] = b""
        print(repr(target), repr(bytearray(b"'")), io.DEFAULT_BUFFER_SIZE)

    target = bytearray(b"ab")
    visited = []
    for item in target:
        visited.append(item)
        if len(visited) == 1:
            target[1:] = b"XY"
    print(visited, bytes(target))

    def truncate(item):
        target[1:] = b""
        return item

    print([truncate(item) for item in target], bytes(target))

    with open(path, "rb") as reader:
        reader.seek(1)
        try:
            reader.seek(-1)
        except OSError as error:
            print(error.args, getattr(error, "errno", None), reader.tell())

    if sys.platform != "win32":
        with open("/dev/null", "rb") as reader:
            print(reader.seek(-1), reader.tell())

    with open(path, "rb") as reader:
        try:
            reader.seek(ClosingOffset(reader))
        except ValueError as error:
            print(error.args, reader.closed)

    lines = path + ".lines"
    with open(lines, "w") as output:
        output.write("first\r\n\nthird\rlast")
    for mode in ("r", "rb"):
        with open(lines, mode) as stream:
            for line in stream:
                print(line)
                break
            print(stream.readline(), [line for line in stream], stream.readline())
        with open(lines, mode) as stream:
            print(iter(stream) is stream, list(zip(stream, stream)), next(stream, "done"))
        for acquire in (iter, lambda stream: zip((), stream)):
            try:
                acquire(stream)
            except ValueError as error:
                assert str(error) == "I/O operation on closed file."
            else:
                raise AssertionError("closed file iterator")
