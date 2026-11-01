def current():
    class Text(str):
        def __str__(self):
            raise AssertionError("string conversion was called")

    class ByteString(bytes):
        pass

    class Mutable(bytearray):
        pass

    class Factory(bytearray):
        def __new__(cls, source):
            return type(source).__name__, bytes(source)

    print(Factory.fromhex("ff"))

    for target in [bytes, bytearray, ByteString, Mutable]:
        method = target.fromhex
        for data in (Text(" FA\t21\v02\f"), b"61 62", bytearray(b"\nAB"), ""):
            result = method(data)
            print(type(result).__name__, bytes(result))
        for data in ("a", "a ", "0z", "xé", "x\ud800", b"x\xff", b"ab\xff", [], None):
            try:
                method(data)
            except (ValueError, TypeError) as exc:
                print(type(exc).__name__, str(exc))
        for args, kwargs in (((), {}), (("aa", "bb"), {}), ((), {"string": "aa"})):
            try:
                method(*args, **kwargs)
            except TypeError as exc:
                print(str(exc))
    print(type(b"".fromhex("aa")).__name__, type(bytearray().fromhex("aa")).__name__)
    print(ByteString(b"").fromhex("00") == b"\0")

    from array import array

    class Initialized(bytearray):
        def __init__(self, data):
            assert type(data) is bytes
            self.decoded = data

    class NeverCoerce:
        def __bytes__(self):
            raise AssertionError("__bytes__ is not the buffer protocol")

    result = Initialized.fromhex("00 fF")
    assert type(result) is Initialized and result.decoded == b"\x00\xff"
    for cls in (bytes, bytearray):
        assert cls.fromhex(array("i", b"31416142")) == b"1AaB"
        assert cls.fromhex("\t\n\v\f\r 0041fF") == b"\x00A\xff"
        assert cls.fromhex(ByteString(b"41")) == cls.fromhex(Mutable(b"41")) == b"A"
        try:
            cls.fromhex(NeverCoerce())
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("accepted non-buffer input")

    first = bytearray.fromhex("ff")
    second = bytearray.fromhex("ff")
    first[0] = 0
    assert second == b"\xff"
