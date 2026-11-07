class Buffer(bytearray):
    pass


def current():
    value = Buffer(b"abcd")
    assert value.pop() == ord("d")
    assert bytearray.pop(value, -2) == ord("b")
    assert value.pop(False) == ord("a")
    assert value == b"c"

    class Grow:
        def __index__(self):
            value.extend(b"de")
            return -2

    assert value.pop(Grow()) == ord("d")
    assert value == b"ce"

    exports = []

    class Export:
        def __index__(self):
            exports.append(memoryview(value))
            return 0

    try:
        value.pop(Export())
    except BufferError:
        assert value == b"ce"
    else:
        raise AssertionError("removed an exported byte")
    try:
        value.pop(9)
    except IndexError as error:
        assert str(error) == "pop index out of range"
    else:
        raise AssertionError("accepted an invalid index")
    exports.pop().release()
    assert value.pop(0) == ord("c")
    assert value.pop() == ord("e")
    try:
        value.pop()
    except IndexError as error:
        assert str(error) == "pop from empty bytearray"
    else:
        raise AssertionError("popped an empty bytearray")
    print("bytearray pop")


current()
