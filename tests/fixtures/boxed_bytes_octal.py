"""Byte-preserving percent fields used by Werkzeug's cookie escape table."""

import sys

class Index:
    def __index__(self):
        return 8


class BadIndex:
    def __index__(self):
        return "8"


class BrokenIndex:
    def __index__(self):
        raise RuntimeError("index failed")


class ConversionError(TypeError):
    pass


class FailingIndex:
    def __index__(self):
        raise ConversionError("index failed")


def current():
    for value in range(256):
        expected = bytes((92, 48 + (value >> 6), 48 + ((value >> 3) & 7), 48 + (value & 7)))
        assert b"\\%03o" % value == expected
    assert b"\xff\x00\x80%%:%o:%03o:\xfe" % (0, 7) == b"\xff\x00\x80%:0:007:\xfe"
    assert b"%#08o" % -9 == b"-0o00011"
    assert b"%-#8.4o" % 9 == b"0o0011  "
    assert b"%+03o %lo %.0o" % (True, Index(), False) == b"+01 10 0"
    assert b"%o" % (2**160 + 7) == b"200000000000000000000000000000000000000000000000000007"
    assert b"100%%" % () == b"100%"
    errors = (
        (b"%o", 1.5, TypeError, "%o format: an integer is required, not float"),
        (b"%o", b"7", TypeError, "%o format: an integer is required, not bytes"),
        (b"%o", BadIndex(), TypeError, "%o format: an integer is required, not BadIndex"),
        (b"%o", BrokenIndex(), RuntimeError, "index failed"),
        (b"%o", FailingIndex(), TypeError, "%o format: an integer is required, not FailingIndex"),
        ("%#x", FailingIndex(), TypeError, "%x format: an integer is required, not FailingIndex"),
        (b"%o %o", (1,), TypeError, "not enough arguments for format string"),
        (b"%o", (1, 2), TypeError, "not all arguments converted during bytes formatting"),
        (b"%03", 8, ValueError, "incomplete format"),
        (b"\xff\x00%q", 8, ValueError, "unsupported format character 'q' (0x71) at index 3"),
        (b"\x00%03\xff", 8, ValueError, "unsupported format character 'ÿ' (0xff) at index 4"),
        (b"%999999999999999999999999o", 8, ValueError, "width too big"),
        (b"%.999999999999999999999999o", 8, ValueError, "prec too big"),
        (b"%.2147483648o", (), ValueError, "prec too big"),
    )
    if sys.maxsize > 2147483647:
        errors += (
            (b"%2147483648o", (), TypeError, "not enough arguments for format string"),
            (b"%2147483648o", None, TypeError, "%o format: an integer is required, not NoneType"),
            (b"%9223372036854775807o", 7, MemoryError, ""),
        )
    for field, value, kind, message in errors:
        try:
            field % value
        except Exception as exc:
            assert type(exc) is kind and str(exc) == message
        else:
            raise AssertionError("missing formatting error")
    try:
        b"%\xff" % 7
    except (ValueError, OverflowError) as exc:
        print(type(exc).__name__, str(exc))
    print("256 octets, binary literals, integer fields and errors matched")
