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

    class Buffer(bytearray):
        pass

    data = Buffer(b"a")
    append = data.append
    assert bytearray.clear(data) is None and data[:] == b""
    assert append(ord("b")) is None and data[:] == b"b"
    assert data.extend(data) is None and data[:] == b"bb"

    class Index:
        def __index__(self):
            data.clear()
            return 255

    assert bytearray.append(data, Index()) is None and data[:] == b"\xff"

    class BufferFlag:
        def __init__(self, operation):
            self.operation = operation

        def __bool__(self):
            self.operation()
            return False

    try:
        b2a_base64(data, newline=BufferFlag(data.clear))
    except BufferError:
        assert data[:] == b"\xff"
    else:
        raise AssertionError("clear resized an exported bytearray")
    data.clear()
    assert b2a_base64(data, newline=BufferFlag(data.clear)) == b""

    data.extend(b"abc")
    copy = bytearray.copy(data)
    assert type(copy) is bytearray and copy == b"abc"
    assert b2a_base64(data, newline=BufferFlag(data.reverse)) == b"Y2Jh"
    assert data[:] == b"cba" and copy == b"abc"

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


def copy_allocation():
    source = bytearray(16381)
    target = bytearray()
    extend = target.extend
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        extend(source)
    except MemoryError as error:
        print(error.args, len(source), len(target))
    extend(source)
    print(len(target))
