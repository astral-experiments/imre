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
