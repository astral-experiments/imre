import errno
import os
import tempfile


class Filename(bytes):
    pass


class Path:
    def __init__(self, name):
        self.name = name
        self.calls = 0

    def __fspath__(self):
        self.calls += 1
        return self.name


def current():
    fd, prefix = tempfile.mkstemp()
    os.close(fd)
    os.unlink(prefix)
    name = Filename(os.fsencode(prefix) + b"-\xff")
    path = Path(name)
    try:
        with open(path, "xb") as stream:
            assert stream.name is name
            assert stream.write(b"first") == 5
        assert path.calls == 1 and stream.name is name
        with open(name, "wb") as stream:
            stream.write(b"A")
        with open(name, "ab") as stream:
            stream.write(b"B")
        with open(path, "rb") as stream:
            assert stream.name is name and stream.mode == "rb"
            assert stream.read() == b"AB"
            assert os.fstat(stream.fileno()).st_size == 2
        assert path.calls == 2 and stream.name is name
        try:
            open(name, "xb")
        except FileExistsError as exc:
            assert exc.errno == errno.EEXIST and exc.filename is name
        else:
            raise AssertionError("exclusive open replaced an existing file")
    finally:
        if os.path.exists(name):
            os.unlink(name)

    try:
        open(path, "rb")
    except FileNotFoundError as exc:
        assert exc.errno == errno.ENOENT and exc.filename is name
    else:
        raise AssertionError("missing file opened")
    assert path.calls == 3

    try:
        open(Path(42), "rb")
    except TypeError:
        pass
    else:
        raise AssertionError("invalid __fspath__ result accepted")
    for bad_path in (b"nul\0byte", Path(b"nul\0byte")):
        try:
            open(bad_path, "rb")
        except ValueError:
            pass
        else:
            raise AssertionError("embedded null byte accepted")

    with open(0, "rb", closefd=False) as stream:
        assert stream.fileno() == 0
    os.fstat(0)
