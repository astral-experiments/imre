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
    for split in ("a:b:c".split, "a:b:c".rsplit):
        print(split(":", maxsplit=Index()))
        try:
            split(":", Broken())
        except ValueError as error:
            print(str(error))
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
    for args, kwargs in (((1, 2, 3), {}), ((), {"a": 1, "b": 2, "c": 3}),
                         ((b".",), {"sep": b"."}), ((), {"unknown": 1})):
        try:
            cookie.rsplit(*args, **kwargs)
        except TypeError as error:
            print(str(error))
