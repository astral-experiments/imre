"""The binary addition used to assemble a fragmented socket request line."""

def add(left, right):
    return left + right


class Chunk:
    pass


def current():
    first = b"raw\x00"
    last = b"\xff\n"
    result = add(first, last)
    assert isinstance(result, bytes) and result == b"raw\x00\xff\n"
    assert add(b"", result) is result and add(result, b"") is result
    assert add(b"", b"") == b""
    mutable = bytearray(last)
    saved = add(first, mutable)
    copied = add(b"", mutable)
    mutable[0] = 0
    assert isinstance(saved, bytes) and saved == result
    assert isinstance(copied, bytes) and copied == last

    for rhs, name in (("text", "str"), (1, "int"), (None, "NoneType"), ([], "list"), (Chunk(), "Chunk")):
        try:
            add(first, rhs)
        except TypeError as exc:
            assert str(exc) == "can't concat " + name + " to bytes"
        else:
            raise AssertionError("bytes accepted an invalid right operand")
    print("binary chunks, ownership, empty pieces and invalid operands matched")
