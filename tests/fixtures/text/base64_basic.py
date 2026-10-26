import base64
import binascii


def decode(data: bytes, alphabet: bytes | None, validate: bool) -> bytes:
    return base64.b64decode(data, alphabet, validate)


def encode(data: bytes, alphabet: bytes | None) -> bytes:
    return base64.b64encode(altchars=alphabet, s=data)


class Flag:
    def __bool__(self) -> bool:
        print("validated")
        return True


def next_flag() -> Flag:
    print("evaluated")
    return Flag()


data = b"\xfb\xef\xff"
for alphabet in (b"-_", b"+/", b"$$", b"\xfb\xff"):
    encoded = encode(data, alphabet)
    print((encoded, decode(encoded, alphabet, False)))
print((encode(data, None), decode(b"++//", None, True)))
print(base64.b64decode(validate=True, altchars="-_", s="--__"))
print(base64.b64decode(""))
for s in ("YTpi", "!!!!", "/w==", "A", "YW=Jj", "é"):
    try:
        print(base64.b64decode(s, validate=True))
    except binascii.Error as e:
        print(("binascii.Error", str(e), repr(e)))
    except ValueError as e:
        print(("ValueError", str(e), repr(e)))

try:
    base64.b64decode("é", "é", next_flag())
except ValueError as e:
    print(str(e))
try:
    base64.b64decode("YQ==", b"!", next_flag())
except AssertionError as e:
    print(str(e))
print(base64.b64decode("YQ==", None, next_flag()))
try:
    base64.b64decode([], b"!")
except TypeError as e:
    print(str(e))
try:
    base64.b64encode("bad", b"!")
except TypeError as e:
    print(str(e))
try:
    base64.b64encode(b"", "π")
except AssertionError as e:
    print(str(e))
try:
    base64.b64encode(b"", "ππ")
except TypeError as e:
    print(str(e))
try:
    base64.b64encode(b"", 0)
except TypeError as e:
    print(str(e))
try:
    base64.b64decode("", "óx")
except ValueError as e:
    print(str(e))
try:
    base64.b64encode(b"", [])
except AssertionError as e:
    print(str(e))
try:
    base64.b64encode(b"", {"a": 1, "b": 2})
except TypeError as e:
    print(str(e))
try:
    base64.b64encode(b"", (1,))
except AssertionError as e:
    print(str(e))
