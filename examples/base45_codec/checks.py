"""Exercise repeated recovery and shared exports without changing upstream code."""

import base45
from base45 import BASE45_DICT, b45decode, b45encode


def check() -> None:
    for value in range(256):
        raw = bytes([value])
        encoded = b45encode(raw)
        print(encoded)
        assert b45decode(encoded) == raw
        assert b45decode(encoded.decode()) == raw

    inputs: list[bytes | str] = [b"BB8", "BB8", "BB8\n"]
    for turn in range(64):
        for malformed in [b"a", b"GGW", b"\xff", b"0"]:
            try:
                b45decode(malformed)
                assert False
            except ValueError as error:
                assert str(error) == "Invalid base45 string"
                assert error.__suppress_context__
                assert error.__cause__ is not None
        for input_value in inputs:
            assert b45decode(input_value) == b"AB"

    BASE45_DICT["0"] = 1
    assert base45.b45decode("00") == b"."
    base45.BASE45_DICT["0"] = 0
    assert b45decode("00") == b"\x00"
    del BASE45_DICT["0"]
    try:
        b45decode("00")
        assert False
    except ValueError as error:
        cause = error.__cause__
        assert cause is not None
        assert isinstance(cause, KeyError)
        assert str(cause) == "'0'"
    BASE45_DICT["0"] = 0
    assert b45decode("00") == b"\x00"
    print("shared exports and recovery passed")


check()
