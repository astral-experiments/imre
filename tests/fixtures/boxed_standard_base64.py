import base64
import binascii


class Text(str):
    def encode(self, encoding):
        print("encoding", encoding)
        return b"YWJj"


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
