import sys


def current():
    assert bytes(divmod(65535, 256)) == b"\xff\xff"
    for value in (1 << 65, -(1 << 65)):
        try:
            bytes([1, value])
        except ValueError as error:
            assert str(error) == "bytes must be in range(0, 256)"
        else:
            raise AssertionError("out-of-range byte accepted")
    for a, b in ((7, -3), (-7, 3), (1 << 150, -17)):
        assert divmod(a, b) == (a // b, a % b)
    for call in (lambda: divmod(1, 0), lambda: divmod(1), lambda: divmod(1, 2, 3),
                 lambda: divmod(1, y=2), lambda: divmod(1, "2"), lambda: divmod(None, 1)):
        try:
            call()
        except (ZeroDivisionError, TypeError) as error:
            print(type(error).__name__, str(error))
        else:
            raise AssertionError("invalid divmod call accepted")
    for n in range(256):
        byte = bytes([n])
        assert n.to_bytes() == byte
        assert n.to_bytes(2, "little") == byte + b"\x00"
        assert int.to_bytes(n, byteorder="big", length=2) == b"\x00" + byte
        assert (n - 128).to_bytes(signed=True) == bytes([(n - 128) & 255])
    assert True.to_bytes() == b"\x01" and False.to_bytes(length=0) == b""
    assert bool.to_bytes is int.to_bytes
    assert bool.to_bytes(True) == b"\x01"
    assert (127).to_bytes(signed=True) == b"\x7f"
    assert (-128).to_bytes(signed=True) == b"\x80"
    assert (-129).to_bytes(2, "little", signed=True) == b"\x7f\xff"
    for size in (8, 9, 32):
        high = 1 << (size * 8 - 1)
        assert high.to_bytes(size, "big") == b"\x80" + bytes(size - 1)
        assert (-high).to_bytes(size, "little", signed=True) == bytes(size - 1) + b"\x80"
        assert (-1).to_bytes(size, "big", signed=True) == b"\xff" * size
        assert (high - 1).to_bytes(size, signed=True) == b"\x7f" + b"\xff" * (size - 1)
        for value in (high, -high - 1):
            try:
                value.to_bytes(size, signed=True)
            except OverflowError as exc:
                assert str(exc) == "int too big to convert"
            else:
                raise AssertionError("signed boundary overflow accepted")

    class Length:
        def __index__(self):
            print("index")
            return 2

    class Signed:
        def __bool__(self):
            print("signed")
            return True

    assert (-2).to_bytes(Length(), "big", signed=Signed()) == b"\xff\xfe"
    for length, order, signed in (
        (0, "big", True),
        (1, "big", False),
        (-1, "big", True),
        (-1, "other", True),
        (1, b"big", True),
        (1, None, True),
        (1 << 100, "big", False),
        (sys.maxsize, "big", False),
        ("1", "big", False),
    ):
        try:
            (-1).to_bytes(length, order, signed=signed)
        except (OverflowError, ValueError, TypeError) as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid conversion")
    for args, keys in (
        ((1, "big", True), {}),
        ((1,), {"length": 2}),
        ((), {"other": True}),
        ((), {"length": 1, "byteorder": "big", "signed": True, "other": 1}),
    ):
        try:
            (1).to_bytes(*args, **keys)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid call")
    for args in ((), (None,), ("x",)):
        try:
            int.to_bytes(*args)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid receiver")
    print("ok")


if __name__ == "__main__":
    current()
