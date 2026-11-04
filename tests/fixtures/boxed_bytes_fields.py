"""Binary percent interpolation with positional and keyed operands."""

import sys
import warnings


class Binary:
    def __bytes__(self):
        return b"a\x00\xff"


class Wrong:
    def __bytes__(self):
        return bytearray(b"x")


class Broken:
    def __bytes__(self):
        raise LookupError("binary conversion")


class BindingError:
    @property
    def __bytes__(self):
        raise RuntimeError("descriptor lookup")


class IndexOnly:
    def __index__(self):
        return 2


class Mapping:
    def __getitem__(self, key):
        assert key == b"(\xff)"
        return b"value"


def current():
    class Child(bytes):
        pass

    value = b"content"
    assert value.__bytes__() is value and bytes.__bytes__.__objclass__ is bytes
    assert bytes.__bytes__.__doc__ == "Convert this value to exact type bytes."
    assert bytes.__bytes__.__text_signature__ == "($self, /)"
    for short in (b"", b"x"):
        item = Child(short)
        assert item.__bytes__() is item.__bytes__()
    child = Child(value)
    result = bytes.__bytes__(child)
    assert type(result) is bytes and result == value
    assert result is not bytes.__bytes__(child)

    class Payload(bytes):
        def rstrip(self):
            return "override"

    value = Payload(b"a ")
    assert bytes.rstrip(value) == b"a" and value.rstrip() == "override"
    assert bytes.decode(value) == "a " and bytes.rfind(value, b"a") == 0
    assert bytes.rsplit(value) == [b"a"]

    class Convert:
        def __bytes__(self):
            return value

        def __iter__(self):
            return iter(b"fallback")

    source = Convert()
    source.__bytes__ = lambda: b"shadow"
    assert bytes(source) is value and bytearray(source) == b"fallback"

    class Text(str):
        def __bytes__(self):
            return b"hook"

    assert bytes(Text("x")) == b"hook" and bytes(Text("x"), "ascii") == b"x"
    for obj, error, message in (
        (Wrong(), TypeError, "__bytes__ returned non-bytes (type bytearray)"),
        (Broken(), LookupError, "binary conversion"),
        (BindingError(), RuntimeError, "descriptor lookup"),
    ):
        try:
            bytes(obj)
        except error as exc:
            assert str(exc) == message
        else:
            raise AssertionError("bytes accepted an invalid conversion")

    operand = Binary()
    operand.__bytes__ = lambda: b"shadow"
    for field in (b"%b", b"%s"):
        assert field % operand == b"a\x00\xff"
        assert field % bytearray(b"a\xff") == b"a\xff"
    assert b"\xff%%:%s:%b:%s:%b\x00" % (b"", b"%s", bytearray(b"\x00"), operand) == b"\xff%::%s:\x00:a\x00\xff\x00"
    assert b"%# +06.2b|%-5.1s|%.s" % (operand, b"xy", b"z") == b"    a\x00|x    |"
    assert b"%*.*s|%*.*b" % (-4, 2, operand, 3, -1, b"x") == b"a\x00  |   "
    assert b"%(x)s:%(x).2b:%%" % {b"x": b"xyz"} == b"xyz:xy:%"
    assert b"%((\xff))s" % Mapping() == b"value"
    assert b"%03o|%s|%b" % (9, b"x", b"y") == b"011|x|y"
    class IntResult:
        def __int__(self):
            return True

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", DeprecationWarning)
        assert b"%d" % IntResult() == b"1"
    assert [warning.category for warning in caught] == [DeprecationWarning]
    assert b"TZif%d" % 3 == b"TZif3"
    assert b"%*.*i|%u|%#x|%X" % (-5, 3, -2, 4.9, 31, 255) == b"-002 |4|0x1f|FF"
    assert b"\xff%(version)d" % {b"version": IndexOnly()} == b"\xff2"
    assert b"%c-%c" % tuple(b"*$") == b"*-$"
    assert b"%#03c|%*.*c|%c" % (Child(b"\xff"), -3, 0, bytearray(b"\x00"), IndexOnly()) == b"  \xff|\x00  |\x02"
    assert b"%(symbol)c" % {b"symbol": 255} == b"\xff"
    assert bytes.__mod__(b"%b", b"x") == b"x"
    assert b"%s".__mod__(operand) == b"a\x00\xff"
    try:
        b"%s".__mod__(other=operand)
    except TypeError as exc:
        assert str(exc) == "wrapper __mod__() takes no keyword arguments"
    else:
        raise AssertionError("keyword argument was accepted")
    errors = (
        (b"%c", -1, OverflowError, "%c arg not in range(256)"),
        (b"%c", 2**100, OverflowError, "%c arg not in range(256)"),
        (b"%c", b"ab", TypeError, "%c requires an integer in range(256) or a single byte, not a bytes object of length 2"),
        (b"%c", bytearray(), TypeError, "%c requires an integer in range(256) or a single byte, not a bytearray object of length 0"),
        (b"%c", memoryview(b"x"), TypeError, "%c requires an integer in range(256) or a single byte, not memoryview"),
        (b"%s", "hi", TypeError, "%b requires a bytes-like object, or an object that implements __bytes__, not 'str'"),
        (b"%b", [1], TypeError, "%b requires a bytes-like object, or an object that implements __bytes__, not 'list'"),
        (b"%b", Wrong(), TypeError, "__bytes__ returned non-bytes (type bytearray)"),
        (b"%.0b", Broken(), LookupError, "binary conversion"),
        (b"%b", BindingError(), TypeError, "%b requires a bytes-like object, or an object that implements __bytes__, not 'BindingError'"),
        (b"%s %b", (b"x",), TypeError, "not enough arguments for format string"),
        (b"%s", (b"x", b"y"), TypeError, "not all arguments converted during bytes formatting"),
        (b"%*s", (IndexOnly(), b"x"), TypeError, "* wants int"),
        (b"%*b", (2**100, b"x"), OverflowError, "Python int too large to convert to C ssize_t"),
        (b"%.*b", (2**100, b"x"), OverflowError, "Python int too large to convert to C int"),
        (b"%(x)s", b"x", TypeError, "format requires a mapping"),
        (b"%(x)s", [], TypeError, "list indices must be integers or slices, not bytes"),
        (b"%(x)s", range(3), TypeError, "range indices must be integers or slices, not bytes"),
        (b"%*q", ("x",), TypeError, "* wants int"),
        (b"%*q", (3,), TypeError, "not enough arguments for format string"),
        (b"%.*", (), TypeError, "not enough arguments for format string"),
        (b"%.*", (2,), ValueError, "incomplete format"),
        (b"%(x)(y)s", {b"x": b"z"}, ValueError, "unsupported format character '(' (0x28) at index 4"),
        (b"%(x", {}, ValueError, "incomplete format key"),
        (b"%(x)s %s", {b"x": b"ok"}, TypeError, "not enough arguments for format string"),
        (b"%(x)", {}, KeyError, "b'x'"),
        (b"%.2147483648b", (), ValueError, "prec too big"),
    )
    if sys.maxsize > 2147483647:
        errors += ((b"%9223372036854775807b", b"x", MemoryError, ""),)
    for field, value, kind, message in errors:
        try:
            field % value
        except Exception as exc:
            assert type(exc) is kind and str(exc) == message, (field, str(exc))
        else:
            raise AssertionError(field)
    print("binary percent fields matched")
