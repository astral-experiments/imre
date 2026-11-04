from binascii import Error, Incomplete, a2b_base64, b2a_base64


class Flag:
    def __init__(self, answer: bool):
        self.answer = answer

    def __bool__(self) -> bool:
        print("flag converted")
        return self.answer


def flag() -> Flag:
    print("flag evaluated")
    return Flag(True)


print((issubclass(Error, ValueError), issubclass(Incomplete, ValueError),
       issubclass(Incomplete, Exception), issubclass(ValueError, Error)))
print((b2a_base64(b""), b2a_base64(b"", newline=False)))
print((a2b_base64("ab==c="), b2a_base64(b"\xfb\xff", newline=False)))
for data in ("=A", "ab=c=", "ab==c=", "abcd=", "!!!!", "A", "é", "YQ=="):
    try:
        print(a2b_base64(data, strict_mode=True))
    except Error as exc:
        print(f"binascii.Error {exc!r}")
    except ValueError as exc:
        print(f"ValueError {exc!r}")
try:
    a2b_base64("ü", strict_mode=flag())
except ValueError as exc:
    print(str(exc))
try:
    a2b_base64(b"!", strict_mode=flag())
except ValueError as exc:
    print(str(exc))
try:
    b2a_base64("bad", newline=flag())
except TypeError as exc:
    print(str(exc))
try:
    a2b_base64(None)
except TypeError as exc:
    print(str(exc))
try:
    b2a_base64([])
except TypeError as exc:
    print(str(exc))
try:
    a2b_base64({})
except TypeError as exc:
    print(str(exc))
try:
    raise Incomplete("partial")
except Error:
    print("caught wrong exception")
except Exception as exc:
    print(repr(exc))
try:
    a2b_base64("AA")
except Exception as exc:
    print((isinstance(exc, Error), isinstance(exc, ValueError),
           isinstance(exc, Incomplete), isinstance(exc, TypeError)))
    print(exc.args)


# A partial group can span ignored bytes before decoding resumes in whole groups.
for split in range(13):
    encoded = b"YWJjZGVmZ2hp"
    separated = encoded[:split] + b"\n!" + encoded[split:]
    assert a2b_base64(separated) == b"abcdefghi"
    try:
        a2b_base64(separated, strict_mode=True)
    except Error as exc:
        assert str(exc) == "Only base64 data is allowed"
    else:
        raise AssertionError("accepted an invalid base64 digit in strict mode")
for suffix in (b"A", b"=A", b"AA=A", b"AA==A", b"AA==\xff", b"AB=="):
    for strict in (True, False):
        try:
            print(a2b_base64(b"YWJj" * 8 + suffix, strict_mode=strict))
        except Error as exc:
            print(str(exc))
