"""Pad immutable and mutable binary sequences using a single byte."""

import sys


class Packet(bytes):
    pass


class Buffer(bytearray):
    pass


def current():
    for base, subtype in ((bytes, Packet), (bytearray, Buffer)):
        value = base(b"xy")
        method = base.ljust
        assert method.__objclass__ is base
        assert method.__text_signature__ == "($self, width, fillchar=b' ', /)"
        assert value.ljust(4) == b"xy  "
        padded = method(value, 3, Buffer(b"\xff"))
        assert type(padded) is base and padded == b"xy\xff"
        unchanged = value.ljust(-1)
        assert unchanged == value and (unchanged is value) == (base is bytes)
        derived = subtype(b"xy")
        copied = derived.ljust(1)
        assert type(copied) is base and copied == derived and copied is not derived
        try:
            derived.ljust(0, memoryview(b"!"))
        except TypeError as error:
            assert str(error) == "ljust() argument 2 must be a byte string of length 1, not memoryview"
        else:
            raise AssertionError("accepted a memoryview as fill")
        try:
            derived.ljust(width=0)
        except TypeError as error:
            assert str(error) == base.__name__ + ".ljust() takes no keyword arguments"
        else:
            raise AssertionError("accepted a width keyword")
        try:
            method(value, sys.maxsize)
        except (MemoryError, OverflowError) as error:
            if base is bytes:
                assert type(error) is OverflowError and str(error) == "byte string is too large"
            else:
                assert type(error) is MemoryError
        else:
            raise AssertionError("accepted an impossible width")

    target = bytearray(b"abc")
    fill = bytearray(b"x")

    class Width:
        def __index__(self):
            target.clear()
            target.append(122)
            fill[0] = 63
            return 3

    assert target.ljust(Width(), fill) == b"z??"
    assert target == b"z" and fill == b"?"
    print("binary ljust matched")
