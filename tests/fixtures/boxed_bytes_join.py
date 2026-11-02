from array import array


def current():
    class Bytes(bytes):
        pass

    one = b"one"
    assert b"/".join([one]) is one
    derived = Bytes(one)
    assert type(bytes.join(derived, [derived])) is bytes
    assert b"".join([]) == b""
    doubles = array("d", [0.0])
    assert b"-".join((bytearray(b"a"), memoryview(b"b"), doubles)) == b"a-b-" + b"\x00" * doubles.itemsize

    class Exporter(bytes):
        def __buffer__(self, flags):
            return memoryview(b"export")

    exporter = Exporter(b"-")
    assert exporter.join([b"a", b"b"]) == b"a-b"
    try:
        result = b"/".join((b"first", exporter))
    except NotImplementedError as error:
        assert "buffer hooks" in str(error)
    else:
        assert result == b"first/export"

    def make():
        buf = bytearray(b"a")
        yield buf
        buf.append(98)
        yield b"c"

    assert b".".join(make()) == b"ab.c"

    def bad():
        yield None
        raise RuntimeError("iterator finished with an error")

    try:
        b"".join(bad())
    except RuntimeError as error:
        print(str(error))
    else:
        raise AssertionError("join consumed a prefix of the iterator")

    view = memoryview(b"")
    view.release()
    for value in (None, (b"ok", "bad"), [view]):
        try:
            b"/".join(value)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("join accepted an invalid input")
