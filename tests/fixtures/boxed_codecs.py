import codecs


class EchoDecoder(codecs.IncrementalDecoder):
    def decode(self, input, final=False):
        return self.errors, input, final


def check_text_encoding_flag():
    entry = codecs.lookup("utf-8")
    assert entry._is_text_encoding is True
    marker = []
    entry._is_text_encoding = marker
    assert codecs.lookup("UTF-8")._is_text_encoding is marker
    assert codecs.lookup("u8")._is_text_encoding is True
    entry._is_text_encoding = None
    assert entry._is_text_encoding is None
    del entry._is_text_encoding
    assert entry._is_text_encoding is True
    try:
        del entry._is_text_encoding
    except AttributeError:
        pass
    else:
        raise AssertionError("deleted the inherited flag")


def check_raw_unicode_escape():
    for alias in ("raw-unicode-escape", "raw_unicode_escape", "RAW Unicode ESCAPE"):
        assert str(b"A\xff\x00", alias) == "A\xff\x00"
        assert br"\u20AC:\U0001f600".decode(alias) == "€:😀"
        assert codecs.decode(br"\\u20ac\\\u20ac", alias) == "\\\\u20ac\\\\€"
        assert bytearray(b"\\x00\\n\\\\q\\").decode(alias) == "\\x00\\n\\\\q\\"
        assert ord(str(br"\udc80", alias)) == 0xdc80
    for wire, start, end, reason in (
        (br"xx\u12QZ", 2, 6, r"truncated \uXXXX escape"),
        (br"\\\U0001", 2, 8, r"truncated \UXXXXXXXX escape"),
        (br"\U00110000", 0, 10, r"\Uxxxxxxxx out of range"),
    ):
        try:
            str(wire, "raw_unicode_escape")
        except UnicodeDecodeError as error:
            assert (error.encoding, error.object, error.start, error.end, error.reason) == (
                "rawunicodeescape", wire, start, end, reason)
        else:
            raise AssertionError(wire)
    assert str(br"\u0!\u20ac", "raw_unicode_escape", "ignore") == "!€"
    assert str(br"\u0!\u20ac", "raw_unicode_escape", "replace") == "\ufffd!€"
    assert str(br"\uQ", "raw_unicode_escape", "backslashreplace") == r"\x5c\x75Q"
    assert str(b"ok", "raw_unicode_escape", "unknown.policy") == "ok"
    def resume(error):
        return "?", -1
    codecs.register_error("test.raw_escape", resume)
    assert codecs.decode(memoryview(br"\u00z"), "raw_unicode_escape", "test.raw_escape") == "?z"

    from collections import namedtuple
    Resume = namedtuple("Resume", "text position")

    class ReplacementBytes(bytes):
        pass

    def change_input(error):
        error.object = ReplacementBytes(br"\u00c4")
        assert codecs.replace_errors(error) == ("\ufffd", 3)
        return Resume("", 0)

    codecs.register_error("test.raw_change_input", change_input)
    assert br"\u3".decode("raw_unicode_escape", "test.raw_change_input") == "Ä"



def check_escape_decode():
    import warnings
    from _codecs import escape_decode

    assert codecs.escape_decode is escape_decode
    payload = bytes(range(256))
    literal = repr(payload)[2:-1].encode("ascii")
    assert escape_decode(memoryview(literal)) == (payload, len(literal))
    assert escape_decode("é\\n", None) == (b"\xc3\xa9\n", 4)
    assert escape_decode(bytearray(br"\1\12\1234"), "unused") == (b"\x01\nS4", 10)
    assert escape_decode(b"a\\\nb") == (b"ab", 4)
    with warnings.catch_warnings():
        warnings.simplefilter("error", DeprecationWarning)
        try:
            escape_decode(br"\777\q")
        except DeprecationWarning as error:
            assert 'b"\\777" is an invalid octal escape sequence' in str(error)
        else:
            raise AssertionError("missing octal escape warning")
    for policy, expected in [("ignore", b"z"), ("replace", b"?z")]:
        assert escape_decode(br"\x4z", policy) == (expected, 4)
    for data, policy, message in [
        (br"a\x", "strict", r"invalid \x escape at position 1"),
        (br"\xk", "nonexistent", "decoding error; unknown error handling code: nonexistent"),
        (b"\\", "ignore", "Trailing \\ in string"),
    ]:
        try:
            escape_decode(data, policy)
        except ValueError as error:
            assert str(error) == message
        else:
            raise AssertionError("expected invalid escape to raise")


def check_boms():
    from codecs import BOM_UTF8, BOM_UTF16_BE, BOM_UTF16_LE, BOM_UTF32_BE, BOM_UTF32_LE

    for name, mark, expected in (
        ("BOM_UTF8", BOM_UTF8, b"\xef\xbb\xbf"),
        ("BOM_UTF16_BE", BOM_UTF16_BE, b"\xfe\xff"),
        ("BOM_UTF16_LE", BOM_UTF16_LE, b"\xff\xfe"),
        ("BOM_UTF32_BE", BOM_UTF32_BE, b"\x00\x00\xfe\xff"),
        ("BOM_UTF32_LE", BOM_UTF32_LE, b"\xff\xfe\x00\x00"),
    ):
        assert type(mark) is bytes and mark == expected
        assert getattr(codecs, name) is mark


def current():
    check_text_encoding_flag()
    check_escape_decode()
    utf16_encoders()
    check_raw_unicode_escape()
    check_boms()
    decoder = codecs.IncrementalDecoder(errors="replace")
    assert decoder.errors == "replace"
    assert decoder.getstate() == (b"", 0)
    assert decoder.reset() is None
    assert decoder.setstate((b"", 0)) is None
    try:
        decoder.decode(b"")
    except NotImplementedError:
        pass
    else:
        raise AssertionError("base decoder accepted input")
    assert EchoDecoder().decode(b"x", final=True) == ("strict", b"x", True)

    class Text(str):
        def encode(self, *args, **kwargs):
            raise AssertionError("codecs.encode called str.encode")

    class Data(bytes):
        def decode(self, *args, **kwargs):
            raise AssertionError("codecs.decode called bytes.decode")

    assert codecs.encode(Text("é")) == b"\xc3\xa9"
    assert codecs.decode(obj=Data(b"\xff"), encoding="latin1") == "ÿ"
    for call in (lambda: codecs.decode(b"x", encoding=None), lambda: codecs.encode(42, "ascii")):
        try:
            call()
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid codec operand accepted")
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
    for name in ("cp437", "437", "ibm437", "cspc8codepage437"):
        text = wire.decode(name)
        assert text[128:131] == "Çüé" and text.encode(name) == wire
        assert codecs.lookup(name).name == "cp437"
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


def utf16_encoders():
    for name, canonical in (('unicodelittleunmarked', 'utf-16-le'), ('utf_16be', 'utf-16-be')):
        text = '🙂\ud800\udcffZ'
        try:
            text.encode(name)
        except UnicodeEncodeError as exc:
            assert exc.object is text
            assert (exc.encoding, exc.start, exc.end, exc.reason) == (canonical, 1, 2, 'surrogates not allowed')
        else:
            raise AssertionError('expected a surrogate encoding error')
        assert text.encode(name, 'ignore') == '🙂Z'.encode(name)
        assert text.encode(name, 'replace') == '🙂??Z'.encode(name)
        assert text.encode(name, 'backslashreplace') == r'🙂\ud800\udcffZ'.encode(name)
        assert text.encode(name, 'namereplace') == text.encode(name, 'backslashreplace')
        assert text.encode(name, 'xmlcharrefreplace') == '🙂&#55296;&#56575;Z'.encode(name)
    assert '\ud800\udcff'.encode('utf-16-le', 'surrogatepass') == b'\x00\xd8\xff\xdc'
    assert '\ud800\udcff'.encode('utf-16-be', 'surrogatepass') == b'\xd8\x00\xdc\xff'
    assert 'ü'.encode('utf-16-le', 'unused handler') == b'\xfc\x00'
