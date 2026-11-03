from array import array
import gc
import io
import os
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


class BaseWriteCases:
    class Override(io.RawIOBase):
        def write(self, data):
            return len(data)

    class Shadow(Override):
        write = None

    class Mixed(io.BufferedIOBase, io.TextIOWrapper):
        pass


def check_exported_writes():
    stream = io.BytesIO(b"id:")
    events = []

    class Chunk:
        def __init__(self, view):
            self.view = view

        def __buffer__(self, flags):
            events.append(flags)
            stream.seek(0, 2)
            return self.view

        def __release_buffer__(self, view):
            events.append((view is self.view, stream.tell()))
            view.release()

    words = array("h", [257, -2, 513])
    data = words.tobytes()
    tail = data[words.itemsize:]
    first = Chunk(memoryview(words)[1:])
    assert stream.write(first) == len(tail)
    stream.writelines([b"|", Chunk(memoryview(words)[:1])])
    expected = b"id:" + tail + b"|" + data[:words.itemsize]
    assert stream.getvalue() == expected
    assert events == [8, (True, 3 + len(tail)), 8, (True, len(expected))]
    words.append(0)

    strided = Chunk(memoryview(b"abcd")[::2])
    try:
        stream.write(strided)
    except BufferError:
        assert events[4:] == [8] and strided.view.tobytes() == b"ac"
        assert stream.getvalue() == expected
    else:
        raise AssertionError("BytesIO accepted a noncontiguous buffer")

    class ClosingChunk(Chunk):
        def __release_buffer__(self, view):
            stream.close()
            view.release()

    assert stream.__init__(ClosingChunk(memoryview(b"new"))) is None
    assert stream.closed

    stream = io.BytesIO()
    assert stream.__setstate__((ClosingChunk(memoryview(b"saved")), 7, {"part": 2})) is None
    assert stream.closed and stream.part == 2


def check_base_writes():
    from _collections_abc import _check_methods

    print(_check_methods(io.IOBase, "write") is NotImplemented,
          _check_methods(io.RawIOBase, "write") is True,
          _check_methods(io.BufferedIOBase, "write") is True,
          _check_methods(BaseWriteCases.Shadow, "write") is NotImplemented)
    raw = BaseWriteCases.Override()
    buffer = io.BytesIO()
    print(raw.write(b"abcd"), buffer.write(b"abc"))
    for method, receiver in ((io.RawIOBase.write, raw),
                             (io.RawIOBase.readinto, raw),
                             (io.BufferedIOBase.write, buffer)):
        for args, kwargs in (((), {}), ((None,), {}), ((1, 2), {}),
                             ((), {"data": 1}), ((1,), {"data": 2})):
            try:
                method(receiver, *args, **kwargs)
            except (NotImplementedError, io.UnsupportedOperation, TypeError) as error:
                print(type(error).__name__, error.args)
            else:
                raise AssertionError("base operation returned a value")
        for args in ((), (1, b"data")):
            try:
                method(*args)
            except TypeError as error:
                print(error.args)
            else:
                raise AssertionError("base operation accepted an invalid receiver")
    print(buffer.getvalue())
    binary = io.BytesIO()
    mixed = BaseWriteCases.Mixed(binary, encoding="utf-8", write_through=True)
    try:
        io.BufferedIOBase.write(mixed, "x")
    except io.UnsupportedOperation as error:
        print(error.args, binary.getvalue())
    else:
        raise AssertionError("base write dispatched to the text wrapper")
    print(io.TextIOWrapper.write(mixed, "abc"), binary.getvalue())

    for stream in (io.RawIOBase(), io.BufferedIOBase()):
        stream.close()
        try:
            stream.write(b"x")
        except (NotImplementedError, io.UnsupportedOperation) as error:
            print(type(error).__name__, error.args)
        else:
            raise AssertionError("closed base operation returned a value")


def check_text_base():
    from _collections_abc import _check_methods

    print(_check_methods(io.TextIOBase, "read", "write"))
    stream = io.StringIO("untouched")
    for transfer, values in ((io.TextIOBase.read, (0, 1 << 80, None)),
                             (io.TextIOBase.write, (b"x", "ok", "a\0b", chr(0xd800)))):
        for value in values:
            try:
                transfer(stream, value)
            except (io.UnsupportedOperation, TypeError, ValueError, OverflowError) as error:
                print(type(error).__name__, str(error))
            else:
                raise AssertionError("base operation returned a value")
    print(stream.getvalue(), stream.tell())
    try:
        io.TextIOBase.readline(stream, ClosingOffset(stream))
    except io.UnsupportedOperation as error:
        print(error.args, stream.closed)


def exercise(path):
    check_exported_writes()
    check_base_writes()
    check_text_base()
    events = []

    class Byte:
        def __index__(self):
            events.append("index")
            return 65

    def elements():
        yield Byte()
        events.append("second")
        yield 256
        events.append("past invalid byte")

    for convert in (bytes, bytearray, bytearray().extend):
        events.clear()
        try:
            convert(elements())
        except ValueError:
            print(events)

    text_path = path + ".text-input"
    with open(text_path, "wb") as writer:
        writer.write(b"A\xe9\r\nZ\n")
    encoded_path = text_path.encode()
    with open(encoded_path, "rt", encoding="latin-1") as reader:
        assert reader.buffer.raw.closefd and reader.name == encoded_path
        assert reader.fileno() == reader.buffer.raw.fileno()
        assert reader.readline() == "A\xe9\n"
        assert reader.read() == "Z\n"
        assert reader.seek(0) == 0
        assert reader.read() == "A\xe9\nZ\n"
    assert reader.closed
    another = open(encoded_path, encoding="latin-1")
    raw = another.buffer.raw
    assert not raw.closed
    del another
    assert raw.closed
    del raw

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

    buffered_path = path + ".buffered"
    with open(buffered_path, "wb", 4) as writer:
        writer.write(b"a")
        writer.write(b"bcd")
        with open(buffered_path, "rb") as reader:
            assert reader.read() == b""
        writer.write(b"e")
        with open(buffered_path, "rb") as reader:
            assert reader.read() == b"abcd"
    with open(buffered_path, "rb") as reader:
        assert reader.read() == b"abcde"

    target_path = path + ".output"
    with open(target_path, "wb") as output:
        assert output.name is target_path and output.mode == "wb"
        alias = output
        print(output.write(b"\x00\xff"), alias.closed)
        with open(target_path, "rb") as reader:
            print(reader.read())
        alias.flush()
        with open(target_path, "rb") as reader:
            assert reader.name is target_path and reader.mode == "rb"
            print(reader.read())
    with open(target_path, "ab") as output:
        print(output.write(bytearray(b"x")))
    with open(target_path, "rb") as reader:
        print(reader.read())
    for handle in (reader, alias):
        for invalid in (lambda: handle.flush(1, 2), lambda: handle.flush(1, unknown=True)):
            try:
                invalid()
            except TypeError as error:
                print(error.args)
            else:
                raise AssertionError("flush accepted extra arguments")
    assert alias.name is target_path and alias.mode == "wb"
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

    class Last:
        def __index__(self):
            return -1

    class Replacement:
        def __index__(self):
            target.extend(b"cd")
            return ord("x")

    target[Last()] = Replacement()
    print(bytes(target))

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

    def bad_lines():
        yield b"prefix"
        gc.collect()
        yield None
        raise AssertionError("read past the rejected item")

    with open(path + ".writelines", "wb") as writer:
        try:
            writer.writelines(bad_lines())
        except TypeError:
            pass
        else:
            raise AssertionError("writelines accepted None")
        assert writer.writelines([b"!"]) is None
        for call, owner in ((lambda: writer.writelines(), "_IOBase"), (writer.writelines, "BufferedWriter")):
            try:
                call()
            except TypeError as error:
                assert str(error) == f"{owner}.writelines() takes exactly one argument (0 given)"
            else:
                raise AssertionError("writelines accepted zero arguments")
    with open(path + ".writelines", "rb") as reader:
        assert reader.read() == b"prefix!"

    with open(path + ".array", "wb") as writer:
        writer.write(b"abc")
    target = array("i", b"x" * 8)
    independent = array("i", target)
    view = memoryview(target)
    with open(path + ".array", "rb") as reader:
        assert reader.readinto(target) == 3
        assert bytes(view) == b"abcxxxxx"
        reader.seek(1)
        assert reader.readinto(view) == 2
        assert reader.readinto(view) == 0
    assert target.tobytes() == b"bccxxxxx"
    view.release()
    assert independent.tobytes() == b"xxxxxxxx"

    read_fd, write_fd = os.pipe()
    try:
        os.set_blocking(read_fd, False)
        with open(read_fd, "rb", closefd=False) as reader:
            dest = bytearray(b"..")
            assert reader.readinto(dest) is None
            os.write(write_fd, b"Q")
            assert reader.readinto(dest) == 1
            assert bytes(dest) == b"Q."
    finally:
        os.close(read_fd)
        os.close(write_fd)

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
