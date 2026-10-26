import codecs


def current():
    entry = codecs.lookup("iso-8859-1")
    for data in (b"", b"GET / HTTP/1.1\r\n", b"X-Test: \xff\x00\r\n", bytes(range(256))):
        text, consumed = entry.decode(data)
        print(text, consumed)
    for name in ("ISO_8859-1", "latin-1", "latin_1", "L1", "cp819"):
        alias = codecs.lookup(name)
        print(alias.name, alias is entry, alias.decode is entry.decode)
    print(codecs.lookup("latin-1") is codecs.lookup("latin_1"))
    decode = entry.decode
    for _ in range(5000):
        _discard = [None]
    print(entry is codecs.lookup("ISO 8859 1"), decode is entry.decode)
    print(decode(bytearray(b"\xff"), "strict"), decode(b"ok", None))
    print(decode(b"ok", "unused-error-handler"))
    for call in (
        lambda: codecs.lookup("unknown-http-codec"),
        lambda: codecs.lookup(""),
        lambda: codecs.lookup("ascii\x00"),
        lambda: codecs.lookup(3),
        lambda: codecs.lookup(None),
        lambda: codecs.lookup(encoding="latin-1"),
        lambda: codecs.lookup(),
        lambda: decode("text"),
        lambda: decode(b"", errors="strict"),
        lambda: decode(b"", "strict\x00"),
        lambda: decode(b"", 3),
        lambda: decode(),
        lambda: decode(b"", "strict", False),
    ):
        try:
            call()
        except (LookupError, TypeError, ValueError) as exc:
            print(type(exc).__name__, str(exc))

    for name, data in (("ascii", b"abc"), ("utf-8", b"\xc3\xa9"),
                       ("utf-8-sig", b"\xef\xbb\xbf\xc3\xa9"), ("cp1251", b"\xff"),
                       ("cp424", b"\x4a")):
        print(codecs.lookup(name).decode(data))
    print(codecs.lookup("utf-8").decode(input=b"\xff", errors="replace"))
