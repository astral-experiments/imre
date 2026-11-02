import os
import tempfile
import warnings


def current():
    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always", RuntimeWarning)
        for fd in (True, 1):
            with os.fdopen(fd, "wb", 0, closefd=False) as stream:
                assert stream.fileno() == 1
    assert len(seen) == 1 and seen[0].category is RuntimeWarning
    assert str(seen[0].message) == "bool is used as a file descriptor"
    assert seen[0].filename == os.fdopen.__code__.co_filename

    with warnings.catch_warnings():
        warnings.simplefilter("error", RuntimeWarning)
        for fd in (False, True):
            try:
                with os.fdopen(fd, encoding="utf-8", closefd=False):
                    pass
            except RuntimeWarning as exc:
                assert exc.args == ("bool is used as a file descriptor",)
            else:
                raise AssertionError("boolean descriptor did not raise RuntimeWarning")

    fd, name = tempfile.mkstemp()
    try:
        with os.fdopen(fd, "wb", closefd=False) as stream:
            stream.write(b"\xc3\xa9\n")
        os.lseek(fd, 0, 0)
        with os.fdopen(fd, encoding="utf-8") as stream:
            print(stream.read(), end="")
        try:
            os.close(fd)
        except OSError as exc:
            print(exc.errno)
        try:
            os.fdopen(name)
        except TypeError as exc:
            print(str(exc))
    finally:
        os.unlink(name)
