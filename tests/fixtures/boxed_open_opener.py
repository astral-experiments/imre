import io
import os
import sys
import warnings


def closed(fd):
    try:
        os.fstat(fd)
    except OSError as exc:
        assert exc.errno == 9
    else:
        raise AssertionError("descriptor survived close")


def main():
    target = "opener-actual"
    spelling = b"opener-requested"
    calls = []
    descriptors = []

    class Path:
        def __fspath__(self):
            calls.append("path")
            return spelling

    def acquire(path, flags):
        assert path == spelling
        calls.append("open")
        fd = os.open(target, flags, 0o600)
        descriptors.append(fd)
        return fd

    for factory in (io.open, open):
        calls.clear()
        with factory(Path(), "w+b", 8, None, None, None, True, acquire) as f:
            assert calls == ["path", "open"]
            assert type(f).__name__ == "BufferedRandom" and f.raw.name == spelling
            assert f.readable() and f.writable() and f.seekable()
            assert f.fileno() == descriptors[-1]
            f.write(b"abc")
            f.seek(0)
            assert f.read() == b"abc"
        closed(descriptors[-1])
        with factory(spelling, "rb", buffering=0, opener=acquire) as raw:
            assert isinstance(raw, io.FileIO) and raw.read() == b"abc"
        closed(descriptors[-1])
        with factory(spelling, "r", opener=acquire) as text:
            assert text.buffer.raw.name == spelling and text.read() == "abc"
        closed(descriptors[-1])

    audit_modes = []
    def audit(event, args):
        if event == "open" and args[0] == spelling:
            audit_modes.append(args[1])
    sys.addaudithook(audit)
    with io.open(spelling, "+br", opener=acquire) as f:
        assert audit_modes == ["r+"] and f.read() == b"abc"
    with open(spelling, "br", buffering=0, opener=acquire) as f:
        assert audit_modes == ["r+", "r"] and f.read() == b"abc"
    text_mode = "".join(["r", "+", "t"])
    fd = os.open(target, os.O_RDWR)
    try:
        with io.open(fd, text_mode, closefd=False, opener=object()) as f:
            assert f.mode == text_mode and f.mode is not text_mode
            assert f.read() == "abc"
        os.fstat(fd)
    finally:
        os.close(fd)

    old_count = len(descriptors)
    with warnings.catch_warnings():
        warnings.simplefilter("error", RuntimeWarning)
        try:
            io.open(spelling, "rb", buffering=1, opener=acquire)
        except RuntimeWarning:
            pass
        else:
            raise AssertionError("binary line buffering warning missing")
    assert len(descriptors) == old_count
    try:
        open(spelling, "wb", encoding="ascii", opener=acquire)
    except ValueError:
        pass
    else:
        raise AssertionError("binary encoding accepted")
    assert len(descriptors) == old_count

    for options, expected in [({"encoding": "no-such-open-codec"}, LookupError),
                              ({"buffering": 0}, ValueError)]:
        try:
            io.open(spelling, "w", opener=acquire, **options)
        except expected:
            pass
        else:
            raise AssertionError("invalid text layer succeeded")
        closed(descriptors[-1])

    read_fd, write_fd = os.pipe()
    try:
        with open(read_fd, "rb", closefd=False, opener=object()) as borrowed:
            assert borrowed.fileno() == read_fd
        os.fstat(read_fd)

        def unseekable(path, flags):
            descriptors.append(os.dup(read_fd))
            return descriptors[-1]

        try:
            io.open(spelling, "r+b", opener=unseekable)
        except io.UnsupportedOperation:
            pass
        else:
            raise AssertionError("update buffer accepted a pipe")
        closed(descriptors[-1])
        os.fstat(read_fd)
    finally:
        os.close(read_fd)
        os.close(write_fd)

    def interrupted(path, flags):
        descriptors.append(os.open(target, os.O_RDONLY))
        raise KeyboardInterrupt("callback owns its fd")

    try:
        open(spelling, "rb", opener=interrupted)
    except KeyboardInterrupt as exc:
        assert str(exc) == "callback owns its fd"
    else:
        raise AssertionError("lost opener exception")
    os.fstat(descriptors[-1])
    os.close(descriptors[-1])
    assert not os.path.exists(spelling)
    assert os.stat(target).st_size == 0
    os.unlink(target)
    if sys.argv[1:]:
        assert os.path.exists(spelling), "opener created the requested spelling"
    print("opener layers and rollback")


main()
