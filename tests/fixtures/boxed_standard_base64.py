import base64
import binascii
from array import array
from io import BytesIO


class Text(str):
    def encode(self, encoding):
        print("encoding", encoding)
        return b"YWJj"


class ShortReads(BytesIO):
    def read(self, size):
        return super().read(min(size, 7))


def current():
    from base64 import b64decode, b64encode
    print(b64encode is base64.b64encode, b64decode.__module__)
    for s in (b"", b"\xfb\xff", bytearray(b"\xfb\xff"), b"Python"):
        for alt in (None, b"-_", bytearray(b"__")):
            coded = b64encode(s, altchars=alt)
            print(coded, b64decode(s=coded, altchars=alt))
    print(b64decode("YWJj", validate=True), b64decode(Text("overridden")))
    for s, alt in ((b"A", None), (b"++==", "z"), (b"", 1), ("\u03a9", None), (None, None), (b"YQ!=", None)):
        try:
            print(b64decode(s, alt, validate=True))
        except (AssertionError, TypeError, ValueError) as exc:
            print(type(exc).__name__, str(exc), type(exc) is binascii.Error)
    for alt in ("a", "ab", b"x", 7):
        try:
            print(b64encode(b"", alt))
        except (AssertionError, TypeError) as exc:
            print(type(exc).__name__, str(exc))
    print(base64.urlsafe_b64decode(base64.urlsafe_b64encode(b"\xfb\xff")))
    for data in (b"", b"x", b"x" * 57, bytearray(b"x" * 58)):
        print(base64.encodebytes(data))
    try:
        base64.encodebytes("text")
    except TypeError as exc:
        print(str(exc))

    encoded = b64encode(b"\xfb\xff", altchars=array("B", b"*$"))
    print(encoded, b64decode(memoryview(encoded), altchars=memoryview(b"*$")))
    print(b64decode(array("B", b"YWJj")), base64.standard_b64decode(memoryview(b"Y.W.J.j.")[::2]))

    stream = BytesIO()
    base64.encode(ShortReads(b"x" * 58), stream)
    print(stream.getvalue())
    stream.seek(0)
    decoded = BytesIO()
    base64.decode(stream, decoded)
    print(decoded.getvalue())
