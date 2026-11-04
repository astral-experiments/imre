import errno
import io
import os


def write(stream, view, on_acquire=None):
    events = []

    class Source:
        def __buffer__(self, flags):
            events.append(flags)
            if on_acquire is not None:
                on_acquire()
            return view

        def __release_buffer__(self, borrowed):
            assert borrowed is view
            events.append("release")
            borrowed.release()

    try:
        result = stream.write(Source())
    except OSError as exc:
        result = (type(exc), exc.errno)
    except (BufferError, ValueError) as exc:
        result = type(exc)
    return result, events


def current():
    reader, writer = os.pipe()
    try:
        os.set_blocking(reader, False)
        with io.FileIO(reader, "wb", closefd=False) as stream:
            released = memoryview(b"x")
            released.release()
            assert write(stream, released) == (ValueError, [0])
            assert write(stream, memoryview(b"abc")[::2]) == (BufferError, [0])
            assert write(stream, memoryview(b"x")) == ((OSError, errno.EBADF), [0, "release"])

            def reopen():
                stream.__init__(writer, "wb", closefd=False)

            source = bytearray(b".ok.")
            assert write(stream, memoryview(source)[1:3], reopen) == (2, [0, "release"])
            source.append(0)
            assert os.read(reader, 2) == b"ok"
            assert write(stream, memoryview(b"x"), stream.close) == (ValueError, [0, "release"])
            assert stream.closed
    finally:
        os.close(reader)
        os.close(writer)
