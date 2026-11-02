from array import array


def current():
    class Buffer(bytearray):
        def __eq__(self, other):
            raise AssertionError("used owner equality")

        def __hash__(self):
            raise AssertionError("used owner hash")

    data = Buffer(b"abc")
    view = memoryview(data)
    assert view.obj is data and not view.readonly
    assert (view.format, view.itemsize, view.nbytes, view.shape) == ("B", 1, 3, (3,))
    assert len(view) == 3 and view == b"abc"
    assert view == memoryview(array("i", [97, 98, 99]))
    try:
        hash(view)
    except ValueError as error:
        assert str(error) == "cannot hash writable memoryview object"
    else:
        raise AssertionError("hashed writable view")

    assert view in bytearray(b"xabcx") and view not in bytearray(b"ab")
    other = memoryview(view)
    view.release()
    data[0] = 65
    assert other.obj is data and bytes(other) == b"Abc"
    try:
        data.append(100)
    except BufferError:
        pass
    else:
        raise AssertionError("resized through a live export")
    other.release()
    data.append(100)
    assert bytes(data) == b"Abcd"
    print("bytearray memoryview")
