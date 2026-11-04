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
    resize_buffer()
    unsigned_cast()
    unsigned_int_cast()
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
    empty = view[:0:2]
    assert not empty.c_contiguous and bytes(empty) == b""
    try:
        BytesIO(b"x").readinto(empty)
    except TypeError:
        pass
    else:
        raise AssertionError("read into a noncontiguous cast slice")
    empty.release()
    assert view[:1:2].c_contiguous
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
    only = part.toreadonly()
    try:
        hash(only)
    except TypeError as err:
        assert "unhashable type" in str(err)
    else:
        raise AssertionError("hashed a view whose exporter is unhashable")
    only.release()
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


def unsigned_int_cast():
    values = array("I", [1, 2, 3, 4])
    size = values.itemsize
    for fmt in ("I", "@I"):
        data = bytearray(b"<" + values.tobytes() + b">")
        with memoryview(data)[1:-1] as span:
            words = span.cast(fmt)
        assert words.obj is data and not words.readonly
        assert (words.format, words.itemsize, words.nbytes) == (fmt, size, 4 * size)
        assert words.shape == (4,) and words.strides == (size,)
        words[0] = 300
        assert data[1:1 + size] == array("I", [300]).tobytes()
        with words.cast("B") as raw:
            raw[size:2 * size] = array("I", [400]).tobytes()
        assert words[1] == 400
        with words[::-2] as stride:
            assert stride.strides == (-2 * size,)
            assert (stride[0], stride[1]) == (4, 400)
            assert stride.tobytes() == array("I", [4, 400]).tobytes()
        words[1:] = words[:-1]
        assert words.tobytes() == array("I", [300, 300, 400, 3]).tobytes()
        words[::2] = words[1::2]
        assert data == bytearray(b"<" + array("I", [300, 300, 3, 3]).tobytes() + b">")
        only = words.toreadonly()
        words.release()
        try:
            only[0] = 0
        except TypeError as error:
            assert str(error) == "cannot modify read-only memory"
        else:
            raise AssertionError("wrote through read-only unsigned int view")
        try:
            data.append(0)
        except BufferError as error:
            assert str(error) == "Existing exports of data: object cannot be re-sized"
        else:
            raise AssertionError("resized exporter with live unsigned int view")
        only.release()
        data.append(0)

    for source, message in [
        (memoryview(values.tobytes())[::2], "memoryview: casts are restricted to C-contiguous views"),
        (memoryview(b"x"), "memoryview: length is not a multiple of itemsize"),
    ]:
        try:
            source.cast("I")
        except TypeError as error:
            assert str(error) == message
        else:
            raise AssertionError("accepted invalid unsigned int cast")


def resize_buffer():
    import sys

    class Buffer(bytearray):
        pass

    data = Buffer(b"abcdef")
    assert bytearray.resize(data, 3) is None
    assert data.resize(6) is None and data == b"abc\x00\x00\x00"

    class Size:
        def __index__(self):
            data.resize(1)
            return 4

    assert data.resize(Size()) is None and data == b"a\x00\x00\x00"
    view = memoryview(data)
    assert data.resize(4) is None
    for n, exception in [(3, BufferError), (5, BufferError), (-1, ValueError),
                         (sys.maxsize, BufferError), (sys.maxsize + 1, OverflowError)]:
        try:
            data.resize(n)
        except exception:
            assert bytes(view) == b"a\x00\x00\x00"
        else:
            raise AssertionError("accepted invalid resize")
    view.release()
    try:
        data.resize(sys.maxsize)
    except MemoryError:
        assert len(data) == 4
    else:
        raise AssertionError("allocated maximum-sized bytearray")
    assert data.resize(0) is None and not data
