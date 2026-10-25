"""Exercise immutable snapshots and both concrete and erased binary stream types."""
import io
from typing import BinaryIO


def as_stream(stream: BinaryIO) -> BinaryIO:
    return stream


def main() -> None:
    original = b"abcdef"
    stream = io.BytesIO(original)
    print(stream.getvalue() is original)
    first = stream.getvalue()
    print(first is stream.getvalue())
    print(repr(stream.read(2)))
    print(repr(stream.getvalue()))
    print(stream.tell())
    alias = as_stream(stream)
    print(alias is stream)
    alias.write(b"XY")
    print(repr(first))
    print(repr(stream.getvalue()))
    stream.seek(9)
    print(repr(stream.getvalue()))
    print(stream.tell())
    stream.write(b"\xff")
    print(repr(stream.getvalue()))
    stream.close()
    print(repr(first))
    try:
        stream.getvalue()
    except ValueError as error:
        print(str(error))
    with io.BytesIO(b"A\nB") as other:
        print(repr(other.getvalue()))
        for line in other:
            print(repr(line))
    print(other.closed)


main()
