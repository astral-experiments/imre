def current():
    decode = int.from_bytes
    assert decode(b"") == 0
    assert decode(bytes=b"\xff\x00", byteorder="little", signed=True) == 255
    assert decode(memoryview(b"\x00\x80"), "little", signed=True) == -32768
    assert decode(bytearray(b"\x80\x00")) == 32768
    assert decode(iter([128, 1]), signed=True) == -32767
    for size in (1, 8, 9, 256):
        for order in ("big", "little"):
            for signed, value in ((True, -(1 << (8*size - 1))), (False, (1 << (8*size)) - 1)):
                assert decode(value.to_bytes(size, order, signed=signed), order, signed=signed) == value

    class Number(int):
        def __init__(self, number):
            self.seen = number

    class Octets:
        def __bytes__(self):
            return b"\xff\xfe"

    n = Number(0).from_bytes(Octets(), signed=True)
    assert type(n) is Number and n == n.seen == -2
    assert False.from_bytes(b"") is False and bool.from_bytes(b"\x01") is True
    assert (9).from_bytes(b"\x17") == 23

    for args, kwargs, error in (
        ((), {}, TypeError), ((b"", "big", True), {}, TypeError),
        ((b"",), {"byteorder": "native"}, ValueError), ((b"", None), {}, TypeError),
        ((2,), {}, TypeError), (("",), {}, TypeError), (([256],), {}, ValueError),
        ((b"",), {"bytes": b""}, TypeError),
    ):
        try:
            decode(*args, **kwargs)
        except error:
            pass
        else:
            raise AssertionError("invalid int.from_bytes call")


if __name__ == "__main__":
    current()
