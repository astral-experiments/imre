import base64


def check():
    class Token(str):
        pass

    class Rewrite(str):
        def encode(self, encoding):
            print("encoding", encoding)
            return bytearray(b"Yg==")

    class TextResult(str):
        def encode(self, encoding):
            return Token("-_8=")

    print(list(base64.urlsafe_b64decode(TextResult("unused"))))

    class Reject(str):
        def encode(self, encoding):
            raise LookupError("from encode")

    for token in (Token("YQ=="), Rewrite("YQ=="), Reject("YQ==")):
        try:
            print(list(base64.urlsafe_b64decode(token)))
        except LookupError as error:
            print(error.args)
    for raw in (b"", b"a", b"ab", b'{"visits":1}', b"\xfb\xff"):
        encoded = base64.urlsafe_b64encode(s=raw)
        print(encoded.decode("ascii"), encoded.rstrip(b"=").decode("ascii"))
        print(list(base64.urlsafe_b64decode(encoded)))
    for text in ("-_8=", "+/8=", "YQ==Yg==", "==YQ==", " !YQ==\x00"):
        print(list(base64.urlsafe_b64decode(s=text)))
    mutable = bytearray(b"\xfb\xff")
    encoded = base64.urlsafe_b64encode(mutable)
    print(encoded.rstrip(bytearray(b"=")).decode("ascii"), list(base64.urlsafe_b64decode(bytearray(encoded))))
    for codec in (base64.urlsafe_b64encode, base64.urlsafe_b64decode):
        for call in (
            lambda: codec(),
            lambda: codec(b"", b""),
            lambda: codec(b"", s=b"", extra=b""),
            lambda: codec(b"", extra=b"", s=b""),
            lambda: codec(S=b""),
        ):
            try:
                call()
            except TypeError as error:
                print(str(error))
    for invalid in ("é", b"x", b"aGVsbG8", 1):
        try:
            base64.urlsafe_b64decode(invalid)
        except (TypeError, ValueError) as error:
            print(str(error))
    try:
        base64.urlsafe_b64encode("text")
    except TypeError as error:
        print(str(error))
    for call in (
        lambda: b"xy".rstrip(b"x", b"y"),
        lambda: b"xy".rstrip(chars=b"y"),
        lambda: b"xy".rstrip(b"x", b"y", extra=b""),
    ):
        try:
            call()
        except TypeError as error:
            print(str(error))
    source = b"\xffa \t\n\v\f\r"
    print(list(source.rstrip()), list(source.rstrip(None)))
    print(source.rstrip(b"") is source, list(b"\xff=\xff".rstrip(b"=\xff")))
    unpadded = b"-_8"
    padded = unpadded + b"=" * (-len(unpadded) % 4)
    print(list(base64.urlsafe_b64decode(padded)))
    print((1 * source) is source)
    print(list(source * -1), list(False * source))

    class Count:
        def __index__(self):
            return 2

    print(list(Count() * b"!"))
    try:
        b"" * (1 << 100)
    except OverflowError as error:
        print(str(error))
