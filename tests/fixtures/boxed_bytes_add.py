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

    left = bytearray(first)
    alias = left
    joined = add(left, last)
    assert isinstance(joined, bytearray) and joined == result
    assert joined is not left and left is alias and left == first
    left += last
    assert left is alias and left == result
    try:
        left += left
    except BufferError as exc:
        assert str(exc) == "Existing exports of data: object cannot be re-sized"
        assert left is alias and left == result
    else:
        raise AssertionError("nonempty bytearray extended itself while exported")
    left += bytearray(left)
    assert left is alias and left == result + result
    holder = [left]
    holder[0] += b"!"
    assert holder[0] is alias and alias == result + result + b"!"
    empty = bytearray()
    empty += empty
    assert empty == b""

    from binascii import b2a_base64

    class ResizeFlag:
        def __init__(self, data, suffix):
            self.data = data
            self.suffix = suffix

        def __bool__(self):
            self.data += self.suffix
            return False

    held = bytearray(b"a")
    assert b2a_base64(held, newline=ResizeFlag(held, b"")) == b"YQ=="
    try:
        b2a_base64(held, newline=ResizeFlag(held, b"b"))
    except BufferError:
        assert held == b"a"
    else:
        raise AssertionError("in-place addition resized an exported bytearray")

    for rhs, name in (("text", "str"), (1, "int"), (None, "NoneType"), ([], "list"), (Chunk(), "Chunk")):
        try:
            add(first, rhs)
        except TypeError as exc:
            assert str(exc) == "can't concat " + name + " to bytes"
        else:
            raise AssertionError("bytes accepted an invalid right operand")
        try:
            add(bytearray(first), rhs)
        except TypeError as exc:
            assert str(exc) == "can't concat " + name + " to bytearray"
        else:
            raise AssertionError("bytearray accepted an invalid right operand")
    print("binary chunks, mutable aliases, empty pieces and invalid operands matched")
