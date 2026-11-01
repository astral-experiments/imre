import os
import tempfile
from io import BytesIO
from tempfile import TemporaryFile


def current():
    buffer = BytesIO(b"first\x00chunk\n")
    with TemporaryFile("wb+") as spill, TemporaryFile() as other:
        alias = spill
        print(spill.fileno() != other.fileno())
        print(spill.write(buffer.getvalue()), other.write(b"other"))
        print(spill.write(b"last\xff"), spill.tell())
        print(spill.seek(0), spill.readline(), spill.read())
        print(other.tell(), other.seek(0), other.read())
        print(spill.flush(), spill.close(), alias.closed, other.closed)
        try:
            alias.flush()
        except ValueError as error:
            print(str(error))
    print(other.closed)

    # The comparison runner removes the created directories and files.
    for directory, prefix in (
        (".", "text-"),
        (b".", b"bytes-"),
        (b".", b"bytes-\xff"),
        (".", os.fsdecode(b"escaped-\xfe")),
    ):
        try:
            folder = tempfile.mkdtemp(prefix=prefix, dir=directory)
        except OSError as error:
            # Some filesystems reject non-UTF-8 directory entries.
            if prefix in ("text-", b"bytes-"):
                raise
            print(type(error).__name__, error.errno, type(error.filename) is type(prefix))
            continue
        print(type(folder) is type(prefix), os.path.exists(folder), not os.path.isfile(folder))
        fd, name = tempfile.mkstemp(prefix=prefix, dir=folder)
        try:
            print(type(name) is type(prefix), os.path.isfile(name), os.get_inheritable(fd))
        finally:
            os.close(fd)
    try:
        tempfile.mkstemp(suffix="mixed", dir=b".")
    except TypeError as error:
        print(str(error))
