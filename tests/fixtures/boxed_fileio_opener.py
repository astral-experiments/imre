import io
import os


def current():
    class Name:
        def __fspath__(self):
            return b"unused-spelling"

    name = Name()
    calls = []

    def opener(argument, flags):
        assert argument is name
        calls.append(flags)
        fd = os.open("opener-target", flags, 0o600)
        os.set_inheritable(fd, True)
        return fd

    try:
        with io.FileIO(name, "wb", opener=opener) as stream:
            assert stream.name is name and not os.get_inheritable(stream.fileno())
            stream.write(b"abc")
        assert calls == [os.O_CREAT | os.O_TRUNC | os.O_WRONLY | os.O_CLOEXEC]
        assert not os.path.exists("unused-spelling")
        with io.FileIO("opener-target", "rb") as stream:
            assert stream.read() == b"abc"
        with io.FileIO("opener-target", "rb") as stream:
            stream.__init__(name, "w", opener=opener)
            assert calls[-1] == os.O_CREAT | os.O_TRUNC | os.O_RDWR | os.O_CLOEXEC
            assert stream.mode == "rb+" and stream.readable() and stream.writable()
            try:
                stream.__init__(name, "a?", opener=opener)
            except ValueError:
                assert stream.closed and stream.mode == "ab+"
            else:
                raise AssertionError("invalid reinitialization mode was accepted")
        inner = os.open("opener-target", os.O_RDONLY)
        outer = os.open("opener-target", os.O_RDONLY)
        try:
            with io.FileIO("opener-target") as stream:
                def nested(argument, flags):
                    stream.__init__(inner, closefd=False)
                    return outer
                stream.__init__(name, opener=nested)
                assert stream.fileno() == outer and stream.name is name and not stream.closefd
            os.fstat(inner)
            os.fstat(outer)
            def nested_error(argument, flags):
                stream.__init__(inner, closefd=False)
                raise KeyError("inner initialized")
            try:
                stream.__init__(name, opener=nested_error)
            except KeyError:
                assert stream.closed and not stream.closefd and stream.name == inner
                os.fstat(inner)
            else:
                raise AssertionError("nested opener error was lost")
        finally:
            os.close(inner)
            os.close(outer)
        for result in (object(), -2, 1 << 100):
            try:
                io.FileIO(name, opener=lambda path, flags: result)
            except (TypeError, ValueError, OverflowError) as exc:
                print(type(exc).__name__, str(exc))
            else:
                raise AssertionError("bad opener result was accepted")

        fd = os.open(".", os.O_RDONLY)
        try:
            io.FileIO(name, opener=lambda path, flags: fd)
        except IsADirectoryError as exc:
            assert exc.filename is name
        try:
            os.fstat(fd)
        except OSError as exc:
            assert exc.errno == 9
        else:
            os.close(fd)
            raise AssertionError("directory fd was not released")
    finally:
        os.unlink("opener-target")
