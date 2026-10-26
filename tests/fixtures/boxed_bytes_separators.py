def current():
    cookie = b"payload.timestamp.signature"
    for needle in (b".", b".time", bytearray(b".sig"), b"", 46, 255):
        print(needle in cookie, needle not in b"")

    class Index:
        def __index__(self):
            return 1

    class Broken:
        def __index__(self):
            raise ValueError("not an index")

    for needle in (Index(), Broken(), None, -1, 256):
        try:
            print(needle in b"\x01")
        except (ValueError, TypeError) as error:
            print(str(error))

    print(cookie.rsplit(sep=bytearray(b"."), maxsplit=Index()))
    print(cookie.rsplit(b"?", 1)[0] is cookie)
    for text, sep in ((b"aaa", b"aa"), (b",,a,", b","), (b"", b"!"),
                      (b" \ta\x0b\xff\x0c ", None), (b"\x85\x1c", None), (b"", None)):
        for limit in (-1, 0, 1, 2):
            print([list(field) for field in text.rsplit(sep, limit)])
    for sep, limit in ((b"", 0), (False, 0), (b".", 1 << 100), (None, "1")):
        try:
            cookie.rsplit(sep, limit)
        except (TypeError, ValueError, OverflowError) as error:
            print(str(error))
