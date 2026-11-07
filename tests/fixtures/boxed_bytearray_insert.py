def current():
    class Buffer(bytearray):
        pass

    data = Buffer(b"abc")
    calls = []

    class Position:
        def __index__(self):
            calls.append("index")
            return -1

    class Byte:
        def __index__(self):
            calls.append("item")
            data.append(100)
            return 88

    assert bytearray.insert(data, Position(), Byte()) is None
    assert calls == ["index", "item"]
    assert data == b"abcXd"

    view = memoryview(data)
    try:
        data.insert(0, 1)
    except BufferError:
        assert bytes(view) == b"abcXd"
    else:
        raise AssertionError("inserted into an exported bytearray")
    view.release()
    data.insert(-100, 0)
    data.insert(100, 255)
    assert data == b"\x00abcXd\xff"
    print("bytearray insert")
