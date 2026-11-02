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
        print(alias.readable(), alias.seek(1))
        destination = bytearray(b"????")
        print(alias.readinto(destination), bytes(destination), alias.tell())
        alias.seek(-2, 2)
        print(alias.readinto(destination), bytes(destination), alias.tell())
        print(alias.readinto(destination), alias.readinto(bytearray()), bytes(destination))
        print(spill.flush(), spill.close(), alias.closed, other.closed)
        try:
            alias.readable()
        except ValueError as error:
            print(str(error))
        try:
            alias.readinto(destination)
        except ValueError as error:
            print(str(error))
        try:
            alias.readinto(b"")
        except TypeError as error:
            print(str(error))
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

    with TemporaryFile("w+", encoding="utf-8", newline="") as text:
        print(text.mode, text.encoding, text.fileno() >= 0)
        print(text.write("é\r\nπ\rx\n"), text.seek(0), list(text))
        text.seek(0)
        print(text.readline(), text.read())
        text.seek(0)
        print(text.buffer.read())
    print(text.closed)
    with TemporaryFile("w+t", encoding="utf-8") as translated:
        translated.write("left\r\nright\r")
        translated.seek(0)
        print(list(translated))

    class Mode(str):
        pass

    with TemporaryFile(Mode("tw"), encoding="latin-1", newline="\r\n") as writer:
        raw = writer.buffer.raw
        print(writer.mode, type(writer.mode) is str, raw.mode, writer.fileno() == raw.name, writer.readable())
        writer.write("é\n")
        writer.flush()
        os.lseek(raw.name, 0, os.SEEK_SET)
        print(os.read(raw.name, 8))
    print(raw.closed)
    for mode, encoding in (("wr", None), ("w", "bad-encoding")):
        try:
            TemporaryFile(mode, encoding=encoding)
        except (ValueError, LookupError) as error:
            print(type(error).__name__, str(error))
