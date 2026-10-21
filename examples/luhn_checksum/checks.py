"""Keep inferred calls and exception recovery live across repeated requests."""

import luhn
from luhn import append, checksum, generate, verify


def check() -> None:
    for turn in range(32):
        assert checksum("") == 0
        assert verify("")
        assert generate("") == 0
        assert append("") == "0"
        for value in ["7992739871", "٠١٢٣٤٥٦٧٨٩", "１２３４５６７８９０"]:
            result = append(value)
            assert result == value + str(generate(value))
            assert verify(result)
            assert luhn.checksum(result) == 0
        try:
            checksum("1x2y")
            assert False
        except ValueError as error:
            assert str(error) == "invalid literal for int() with base 10: 'x'"
            assert error.__cause__ is None
            assert not error.__suppress_context__
        assert verify("79927398713")
    print("inferred calls and recovery passed")


check()
