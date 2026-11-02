import os
import tempfile


def current():
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
