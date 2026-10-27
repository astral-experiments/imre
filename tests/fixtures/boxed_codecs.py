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
    for decode_invalid in (lambda: b"ab\xffcd".decode("utf-8"),
                           lambda: codecs.lookup("ascii").decode(b"ab\xffcd")):
        try:
            decode_invalid()
        except UnicodeDecodeError as exc:
            print(exc.encoding, exc.object, exc.start, exc.end, exc.reason)
            print(str(exc), repr(exc), exc.args, exc.object is exc.args[1])

    seen = []
    def retry(exc):
        resume = exc.end
        if seen:
            previous, args = seen[0]
            print(exc is previous, exc.args is args, exc.object, exc.start, exc.end, str(exc))
        else:
            seen.append((exc, exc.args))
            exc.object = b"\xffB\xfe"
            exc.encoding = "changed"
            exc.reason = "retry"
            exc.start = 0
            exc.end = 1
        return "!", resume
    codecs.register_error("boxed_codec_retry", retry)
    print(b"\xffA\xff".decode("utf-8", "boxed_codec_retry"), seen[0][0].args)

    wire = bytes(range(256))
    text = entry.decode(wire)[0]
    for name in ("latin-1", "LATIN_1", "iso-8859-1", "cp819"):
        print(text.encode(name) == wire, text.encode(name, "strict") == wire)
    print("HTTP/1.0 200 OK\r\n".encode("latin-1"), "X-Test: café ÿ\r\n".encode("latin-1"))
    print("".encode("latin-1"), "\x00".encode("latin-1"), "€ÿ".encode("latin-1", "replace"))
    for value in ("a€b", "a€☃b", "東京"):
        try:
            value.encode("latin-1")
        except UnicodeEncodeError as exc:
            print(str(exc))
        try:
            value.encode("latin-1", "strict")
        except UnicodeEncodeError as exc:
            print(str(exc))

    source = bytearray(b"x\xe2(\xa1")
    spans = []

    def escape(exc):
        spans.append((exc.start, exc.end, exc.reason))
        try:
            source.extend(b"!")
        except BufferError as e:
            print(type(e).__name__, str(e))
        return "[" + str(exc.start) + "]", exc.end

    codecs.register_error("test.native_decode", escape)
    print(codecs.lookup_error("test.native_decode") is escape)
    print(source.decode("utf-8", "test.native_decode"), spans)
    source.extend(b"!")
    print(codecs.lookup("utf8").decode(b"", "missing.handler"))
    codecs.register_error("test.native_decode", codecs.backslashreplace_errors)
    print(codecs.lookup("utf8").decode(b"\xf0\x9f", "test.native_decode"))
    codecs.register_error("test.native_decode", lambda e: ("?", -1))
    print(str(b"\xffZ", "utf8", "test.native_decode"))
    for invalid in ((b"?", 1), ("?", 3)):
        codecs.register_error("test.native_decode", lambda e: invalid)
        try:
            b"\xffZ".decode("utf8", "test.native_decode")
        except (TypeError, IndexError) as e:
            print(type(e).__name__, str(e))

    class PolicyName(str):
        pass

    named = PolicyName("test.native_decode")
    print(codecs.lookup_error(named) is codecs.lookup_error(str(named)))
    try:
        b"".decode("utf-8", "bad\x00name")
    except ValueError as e:
        print(type(e).__name__, str(e))

    from urllib.parse import unquote
    codecs.register_error("test.url", lambda e: ("?", e.end))
    print(unquote("a%E2%28%A1é", "utf8", "test.url"))

    seen = []
    def strict_alias(exc):
        seen.append(exc)
        return codecs.strict_errors(exc)
    codecs.register_error("test.strict_alias", strict_alias)
    try:
        b"\xff".decode("utf8", "test.strict_alias")
    except UnicodeDecodeError as e:
        print(e is seen[0], str(e))
