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
    unsigned_cast()
    print("bytearray memoryview")


def unsigned_cast():
    from io import BytesIO

    data = array("i", b"AAAABBBBCCCCDDDD")
    original = memoryview(data)
    sub = original[1:3]
    view = sub.cast(format="B", shape=[8])
    original.release()
    sub.release()
    assert view.obj is data and view.format == "B" and view.shape == (8,)
    assert view.itemsize == 1 and view.nbytes == 8 and view.strides == (1,)
    assert view[:4] == b"BBBB" and bytes(view[4:]) == b"CCCC"
    assert BytesIO(b"XYZ").readinto(view[2:5]) == 3
    assert data.tobytes() == b"AAAABBXYZCCCDDDD"
    assert view[::2] == b"BXZC"
    part = view[1:4].cast("@B")
    assert part.format == "@B" and part[0] == ord("B")
    assert part.shape == (3,) and part.obj is data
    view.release()
    part[0] = ord("P")
    part[1:] = b"QR"
    assert data.tobytes() == b"AAAABPQRZCCCDDDD"
    try:
        data.append(1)
    except BufferError:
        pass
    else:
        raise AssertionError("cast slice failed to hold exporter")
    part.release()
    data.append(1)
    try:
        part.cast("B")
    except ValueError:
        pass
    else:
        raise AssertionError("cast released view")
    ro = memoryview(b"abcd")[1:].cast("B")
    assert ro.readonly and ro == b"bcd" and hash(ro) == hash(b"bcd")
    try:
        ro[0] = 0
    except TypeError:
        pass
    else:
        raise AssertionError("wrote readonly cast")
    for source, args, error in [(memoryview(data)[::2], ("B",), TypeError),
                                (ro, ("=B",), ValueError), (ro, ("B", [2]), TypeError)]:
        try:
            source.cast(*args)
        except error:
            pass
        else:
            raise AssertionError("accepted invalid cast")
