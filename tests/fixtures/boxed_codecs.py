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


def check_big5():
    import io
    entry = codecs.lookup("csbig5")
    assert entry.name == "big5"
    wire = bytes.fromhex("a2cc a2ce a1fe a240")
    text = "十卅／＼"
    assert entry.decode(wire) == (text, 8)
    assert entry.encode(text) == (bytes.fromhex("a451 a4ca a241 a242"), 4)
    reader = entry.streamreader(io.BytesIO(bytes.fromhex("a578 a55f 0a")))
    assert list(reader) == ["台北\n"]
    decoder = entry.incrementaldecoder()
    assert decoder.decode(b"\x80") == ""
    try:
        decoder.decode(b"", True)
    except UnicodeDecodeError as error:
        assert (error.start, error.end, error.reason) == (0, 1, "incomplete multibyte sequence")
    else:
        raise AssertionError("accepted a truncated Big5 lead")
    assert decoder.getstate() == (b"\x80", 0)
    decoder.errors = "replace"
    assert decoder.decode(b" ", True) == "� "

    def bad_position(error):
        return "?", 2**100

    codecs.register_error("big5.badposition", bad_position)
    for function, value in ((entry.encode, "😀"), (entry.decode, b"\xff")):
        try:
            function(value, "big5.badposition")
        except IndexError as error:
            assert str(error) == "position -1 from error handler out of bounds"
        else:
            raise AssertionError("accepted an overflowing multibyte callback position")


def check_shift_jis():
    entry = codecs.lookup("sjis")
    assert entry.name == "shift_jis"
    text = "これは古い日本語です。\n"
    wire = bytes.fromhex("82b182ea82cd8cc382a293fa967b8cea82c582b781420a")
    assert text.encode("shift-jis") == wire
    assert wire.decode("csshiftjis") == text
    assert entry.encode("¥‾ｱ〜") == (b"\\~\xb1\x81\x60", 4)
    assert entry.decode(b"\\~\xb1\x81\x60") == ("\\~ｱ〜", 5)
    for data, reason in ((b"A\x81", "incomplete multibyte sequence"),
                         (b"A\x87@", "illegal multibyte sequence")):
        try:
            data.decode("shift_jis")
        except UnicodeDecodeError as error:
            assert (error.encoding, error.object, error.start, error.end, error.reason) == (
                "shift_jis", data, 1, 2, reason)
        else:
            raise AssertionError(data)
    assert b"A\x87@".decode("shift_jis", "replace") == "A\ufffd@"
    try:
        "x①①".encode("shift_jis")
    except UnicodeEncodeError as error:
        assert (error.encoding, error.object, error.start, error.end, error.reason) == (
            "shift_jis", "x①①", 1, 2, "illegal multibyte sequence")
    else:
        raise AssertionError("accepted a CP932 extension")

    source = "A☃xB"
    errors = []

    def replace_snowman(error):
        errors.append(error)
        return replacement, error.end

    codecs.register_error("test.sjis_replace", replace_snowman)
    replacement = "Q日"
    assert source.encode("shift_jis", "test.sjis_replace") == b"AQ\x93\xfaxB"
    replacement = "Q😀"
    try:
        source.encode("shift_jis", "test.sjis_replace")
    except UnicodeEncodeError as error:
        assert error.object is replacement
        assert (error.encoding, error.start, error.end, error.reason) == (
            "shift_jis", 1, 2, "illegal multibyte sequence")
    else:
        raise AssertionError("accepted an unencodable replacement")
    assert len(errors) == 2
    assert all(error.object is source and (error.start, error.end) == (1, 2) for error in errors)

    def second_handler(error):
        codecs.register_error("test.sjis_switch", codecs.replace_errors)
        return b"B", error.end

    def switch_handler(error):
        codecs.register_error("test.sjis_switch", second_handler)
        return b"A", error.end

    codecs.register_error("test.sjis_switch", switch_handler)
    assert "☃😀☃".encode("shift_jis", "test.sjis_switch") == b"AB?"


def check_raw_unicode_escape():
    source = "\0\n\\u1234\x1aÿĀ€\ud800\udfff😀"
    encoded = b"\0\n\\u1234\x1a\xff" + br"\u0100\u20ac\ud800\udfff\U0001f600"
    for alias in ("raw-unicode-escape", "raw_unicode_escape", "RAW Unicode ESCAPE"):
        assert source.encode(alias, "unknown.policy") == encoded
        assert codecs.encode(source, alias) == encoded
        assert str(b"A\xff\x00", alias) == "A\xff\x00"
        assert br"\u20AC:\U0001f600".decode(alias) == "€:😀"
        assert codecs.decode(br"\\u20ac\\\u20ac", alias) == "\\\\u20ac\\\\€"
        assert bytearray(b"\\x00\\n\\\\q\\").decode(alias) == "\\x00\\n\\\\q\\"
        assert ord(str(br"\udc80", alias)) == 0xdc80
    try:
        codecs.encode(b"x", "raw-unicode-escape")
    except TypeError as error:
        assert str(error) == "raw_unicode_escape_encode() argument 1 must be str, not bytes"
        assert error.__notes__ == ["encoding with 'raw-unicode-escape' codec failed"]
    else:
        raise AssertionError("accepted bytes for a text encoder")
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
    from sys import byteorder

    for name, mark, expected in (
        ("BOM_UTF8", BOM_UTF8, b"\xef\xbb\xbf"),
        ("BOM_UTF16_BE", BOM_UTF16_BE, b"\xfe\xff"),
        ("BOM_UTF16_LE", BOM_UTF16_LE, b"\xff\xfe"),
        ("BOM_UTF32_BE", BOM_UTF32_BE, b"\x00\x00\xfe\xff"),
        ("BOM_UTF32_LE", BOM_UTF32_LE, b"\xff\xfe\x00\x00"),
    ):
        assert type(mark) is bytes and mark == expected
        assert getattr(codecs, name) is mark

    assert codecs.BOM_LE is codecs.BOM32_LE is BOM_UTF16_LE
    assert codecs.BOM_BE is codecs.BOM32_BE is BOM_UTF16_BE
    assert codecs.BOM64_LE is BOM_UTF32_LE
    assert codecs.BOM64_BE is BOM_UTF32_BE
    assert codecs.BOM is codecs.BOM_UTF16 is (BOM_UTF16_LE if byteorder == "little" else BOM_UTF16_BE)
    assert codecs.BOM_UTF32 is (BOM_UTF32_LE if byteorder == "little" else BOM_UTF32_BE)


def check_utf8_incremental_encoder():
    from encodings.utf_8 import IncrementalEncoder

    factory = codecs.getincrementalencoder("u8")
    assert factory is codecs.lookup("UTF-8").incrementalencoder is IncrementalEncoder
    encoder = factory()
    assert encoder.encode("Aé") == b"A\xc3\xa9"
    assert encoder.getstate() == 0
    encoder.setstate(0)
    assert encoder.encode("€", final=True) == b"\xe2\x82\xac"
    assert encoder.reset() is None
    assert encoder.getstate() == 0
    encoder.errors = "replace"
    assert encoder.encode("\ud800", final=True) == b"?"
    assert factory("ignore").encode("\ud800Z") == b"Z"


def check_single_byte_stream_readers():
    import encodings
    import sys
    from encodings import ascii, latin_1
    from io import BytesIO

    for name, provider in (("ascii", ascii), ("L1", latin_1)):
        registry = codecs.lookup(name)
        entry = provider.getregentry()
        assert codecs.getreader(name) is entry.streamreader is provider.StreamReader
        assert provider.StreamReader.__bases__ == (provider.Codec, codecs.StreamReader)
        assert registry.encode is entry.encode is provider.Codec.encode
        assert registry.decode is entry.decode is provider.StreamReader.decode
        # A default search cache hit must not import the provider or its parent again.
        for key in (provider.__name__, "encodings"):
            saved = sys.modules[key]
            sys.modules[key] = None
            try:
                assert encodings.search_function(name.lower()) is registry
            finally:
                sys.modules[key] = saved
    assert ascii.StreamReader is not latin_1.StreamReader
    latin = latin_1.StreamReader(BytesIO(b"A\xffB"))
    assert latin.read(2) == "Aÿ"
    latin.seek(1)
    assert latin.read() == "ÿB" and latin.read() == ""

    assert codecs.getreader("ascii") is ascii.StreamReader
    factory = codecs.getreader("ascii")
    assert factory is codecs.lookup("us-ascii").streamreader
    raw = BytesIO(b"A\xffB")
    reader = factory(raw, "replace")
    assert reader.read(1) == "A" and raw.tell() == 1
    assert reader.read(1) == "\ufffd" and raw.tell() == 2
    assert reader.read() == "B"
    assert reader.read() == reader.read() == ""


def check_utf8_provider_entries():
    from encodings import utf_8, utf_8_sig

    for provider, name in ((utf_8, "utf-8"), (utf_8_sig, "utf-8-sig")):
        entry = provider.getregentry()
        registered = codecs.lookup(name)
        assert registered.encode is entry.encode is provider.encode
        assert registered.decode is entry.decode is provider.decode
        assert registered.streamreader is entry.streamreader is provider.StreamReader
        assert registered.streamwriter is entry.streamwriter is provider.StreamWriter
        assert registered.incrementalencoder is entry.incrementalencoder is provider.IncrementalEncoder
        assert registered.incrementaldecoder is entry.incrementaldecoder is provider.IncrementalDecoder


def check_utf8_stream_readers():
    from io import BytesIO

    data = codecs.BOM_UTF8 + "AΩ".encode() + codecs.BOM_UTF8
    for name, expected in (("utf8", "\ufeffAΩ\ufeff"), ("utf_8_sig", "AΩ\ufeff")):
        factory = codecs.getreader(name)
        assert factory is codecs.lookup(name).streamreader
        reader = factory(BytesIO(data))
        assert reader.read(1) + reader.read() == expected
        reader.seek(0)
        assert reader.read() == expected


def check_charmap_undefined_bytes():
    for name, value in (("cp1251", 0x98), ("cp1252", 0x81), ("cp424", 0x80)):
        raw = bytes([value])
        assert ord(raw.decode(name, "surrogateescape")) == 0xdc00 + value
    raw = b"\x80\x70"
    try:
        raw.decode("cp424", "surrogateescape")
    except UnicodeDecodeError as exc:
        assert (exc.encoding, exc.object, exc.start, exc.end) == ("charmap", raw, 1, 2)
    else:
        raise AssertionError("surrogate-escaped an unmapped ASCII byte")

    def replace_byte(exc):
        assert exc.encoding == "charmap" and exc.object == raw
        return str(exc.start), exc.end

    for errors in ("strict", "replace", "ignore", "surrogateescape"):
        previous = codecs.lookup_error(errors)
        codecs.register_error(errors, replace_byte)
        try:
            assert raw.decode("cp424", errors) == "01"
        finally:
            codecs.register_error(errors, previous)


def check_charmap_factories():
    from encodings import cp1252
    from io import BytesIO

    entry = codecs.lookup("windows-1252")
    assert entry.incrementalencoder is codecs.getincrementalencoder("cp1252") is cp1252.IncrementalEncoder
    assert entry.streamreader is codecs.getreader("cp1252") is cp1252.StreamReader
    encoder = entry.incrementalencoder("replace")
    assert encoder.encode("€") + encoder.encode("☃", final=True) == b"\x80?"
    reader = entry.streamreader(BytesIO(b"\x80\x81"), "replace")
    assert reader.read(1) == "€"
    assert reader.read() == "\ufffd"
    reader.seek(0)
    assert reader.read() == "€\ufffd"


def check_module_factory_results():
    from encodings import latin_1

    calls = []
    def invalid():
        calls.append("invalid")
        return None

    def legacy():
        calls.append("legacy")
        return codecs.ascii_encode, codecs.ascii_decode, None, None

    original = latin_1.getregentry
    try:
        latin_1.getregentry = invalid
        try:
            codecs.lookup("csisolatin1")
        except TypeError as error:
            assert str(error) == "object of type 'NoneType' has no len()"
        else:
            raise AssertionError("accepted an invalid module factory result")
        latin_1.getregentry = legacy
        info = codecs.lookup("csisolatin1")
        assert isinstance(info, codecs.CodecInfo) and info.name == "latin_1"
        assert codecs.getencoder("csisolatin1") is info.encode is codecs.ascii_encode
        assert info.encode("é", "replace") == (b"?", 1)
        assert calls == ["invalid", "legacy"]
    finally:
        latin_1.getregentry = original


def check_cached_package_callback():
    import encodings
    import sys
    from encodings import ascii

    search = encodings.search_function
    saved = sys.modules["encodings"]
    sys.modules["encodings"] = None
    try:
        entry = search("us_ascii!")
        assert entry.decode is ascii.Codec.decode
        assert search("us_ascii!") is entry
    finally:
        sys.modules["encodings"] = saved


def check_reentrant_codec_factory():
    import encodings
    from encodings import latin_1

    original = latin_1.getregentry
    expected = original()
    seen = []

    class Factory:
        def __call__(self):
            latin_1.getregentry = original
            return expected

        def __del__(self):
            seen.append(encodings.search_function("iso_ir_100"))

    try:
        latin_1.getregentry = Factory()
        assert codecs.lookup("iso_ir_100") is expected
        assert seen == [expected] and seen[0] is expected
    finally:
        latin_1.getregentry = original


def check_incremental_decoder_factory():
    from encodings.utf_8 import IncrementalDecoder

    import encodings.latin_1
    import gc
    latin_decoder = encodings.latin_1.IncrementalDecoder
    del encodings.latin_1.IncrementalDecoder
    try:
        codecs.lookup("L1")
    except NameError as error:
        assert error.name == "IncrementalDecoder"
    else:
        raise AssertionError("missing Latin-1 decoder")
    finally:
        encodings.latin_1.IncrementalDecoder = latin_decoder
    gc.collect()
    latin = codecs.lookup("L1")
    assert latin.decode(b"\xc7") == ("Ç", 1) and latin.incrementaldecoder is latin_decoder

    entry = codecs.lookup("utf-8")
    assert codecs.getincrementaldecoder("u8") is entry.incrementaldecoder is IncrementalDecoder
    decoder = IncrementalDecoder()
    assert decoder.decode(b"A\xe2") == "A"
    assert decoder.getstate() == (b"\xe2", 0)
    decoder.setstate((b"\xe2\x82", 0))
    assert decoder.decode(b"\xac", True) == "€"
    for name in ("ascii", "latin-1", "cp1251", "cp424", "cp437"):
        decoder = codecs.getincrementaldecoder(name)()
        assert decoder.decode(b"x", True) == codecs.lookup(name).decode(b"x")[0]
    decoder = codecs.getincrementaldecoder("cp437")()
    lookup = codecs.lookup
    codecs.lookup = None
    try:
        assert decoder.decode(b"\x82") == "é"
    finally:
        codecs.lookup = lookup
    def search(name):
        if name == "incremental_test":
            return codecs.CodecInfo(None, None, incrementaldecoder=IncrementalDecoder)

    codecs.register(search)
    try:
        assert codecs.getincrementaldecoder("incremental-test") is IncrementalDecoder
    finally:
        codecs.unregister(search)
    decoder = IncrementalDecoder("surrogatepass")
    assert decoder.decode(b"\xed\xa0") == ""
    assert ord(decoder.decode(b"\x80", True)) == 0xd800
    decoder.reset()
    assert decoder.getstate() == (b"", 0)

    def replace_input(error):
        assert error.object == b"\xff\xe2"
        error.object = b"ABC\xe2"
        return "!", 1

    codecs.register_error("test.incremental_input", replace_input)
    assert codecs.utf_8_decode(b"\xff\xe2", "test.incremental_input", False) == ("!BC", 3)
    entry.incrementaldecoder = None
    try:
        codecs.getincrementaldecoder("UTF-8")
    except LookupError as error:
        assert error.args == ("UTF-8",)
    else:
        raise AssertionError("missing incremental decoder")
    finally:
        entry.incrementaldecoder = IncrementalDecoder


def check_utf8_error_handler():
    def replace_surrogate(error):
        return "?", error.end

    codecs.register_error("test.utf8_scalar", replace_surrogate)
    for encoding in ("utf-8", "utf-8-sig"):
        assert "é東🚉".encode(encoding, "test.utf8_scalar") == "é東🚉".encode(encoding)
        assert "A\ud800B".encode(encoding, "test.utf8_scalar") == "A?B".encode(encoding)


def check_utf8_buffer_slice():
    from array import array

    assert codecs.utf_8_decode(array("B", b"data")) == ("data", 4)
    source = memoryview(array("B", b"\xffdata\xff"))
    assert codecs.utf_8_decode(source[1:5].cast("I")) == ("data", 4)
    try:
        codecs.utf_8_decode(source[1:5:2])
    except BufferError:
        pass
    else:
        raise AssertionError("decoded a non-contiguous array view")


def current():
    check_utf8_error_handler()
    check_utf8_buffer_slice()
    check_utf8_incremental_encoder()
    check_utf8_provider_entries()
    check_utf8_stream_readers()
    check_charmap_factories()
    check_charmap_undefined_bytes()
    check_incremental_decoder_factory()
    check_module_factory_results()
    check_reentrant_codec_factory()
    check_cached_package_callback()
    check_single_byte_stream_readers()
    check_text_encoding_flag()
    check_escape_decode()
    fixed_endian_encoders()
    fixed_endian_streams()
    utf16_extended_decoder()
    check_shift_jis()
    check_big5()
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

    koi8_r = codecs.lookup("cskoi8r")
    assert koi8_r.name == "koi8-r"
    assert koi8_r.incrementaldecoder().decode(b"\xa4", final=True) == "╓"
    assert wire.decode("KOI8-R").encode("cskoi8r") == wire

    from encodings import koi8_r as provider
    fresh = provider.getregentry()
    assert fresh is not provider.getregentry() and fresh is not koi8_r
    assert fresh.name == koi8_r.name and fresh.encode("╓") == (b"\xa4", 1)
    assert fresh.decode(b"\xa4") == ("╓", 1)
    assert fresh.encode.__self__ is not fresh.decode.__self__
    assert fresh.encode.__self__.__class__ is fresh.decode.__self__.__class__ is provider.Codec
    assert fresh.incrementalencoder is provider.IncrementalEncoder
    assert fresh.incrementaldecoder is provider.IncrementalDecoder
    assert fresh.streamwriter is provider.StreamWriter and fresh.streamreader is provider.StreamReader

    from encodings.iso8859_2 import Codec as Latin2
    latin2 = codecs.lookup("iso.ir.101")
    assert latin2.decode.__self__.__class__ is Latin2
    assert latin2.decode.__func__ is Latin2.decode
    assert Latin2.__bases__ == (codecs.Codec,)
    assert latin2.decode(b"\xa1") == ("Ą", 1)
    from encodings.iso8859_2 import IncrementalEncoder, StreamReader
    from io import BytesIO

    assert codecs.getincrementalencoder("l2") is latin2.incrementalencoder is IncrementalEncoder
    assert codecs.getreader("latin2") is latin2.streamreader is StreamReader
    text = StreamReader(BytesIO(wire)).read()
    assert text[0xA1] == "Ą" and IncrementalEncoder().encode(text, final=True) == wire

    entry_koi8 = codecs.lookup("KOI8_U")
    assert entry_koi8.name == "koi8-u"
    assert entry_koi8.decode(bytes.fromhex("a4 b4 ad bd a6 b6 a7 b7")) == ("єЄґҐіІїЇ", 8)
    assert wire.decode("koi8-u").encode("koi8-u") == wire
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


def fixed_endian_encoders():
    for name, canonical in (
        ('unicodelittleunmarked', 'utf-16-le'), ('utf_16be', 'utf-16-be'),
        ('utf_32le', 'utf-32-le'), ('utf-32be', 'utf-32-be'),
    ):
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

    assert '\ud800\udcff'.encode('utf-32-le', 'surrogatepass') == b'\x00\xd8\x00\x00\xff\xdc\x00\x00'
    assert '\ud800\udcff'.encode('utf-32-be', 'surrogatepass') == b'\x00\x00\xd8\x00\x00\x00\xdc\xff'


def utf16_extended_decoder():
    from _codecs import utf_16_ex_decode

    assert utf_16_ex_decode is codecs.utf_16_ex_decode
    assert utf_16_ex_decode(b"\xfe\xff\xd8\x3d") == ("", 2, 1)
    assert utf_16_ex_decode(b"\xd8\x3d\xde\x00", None, 1) == ("😀", 4, 1)
    assert utf_16_ex_decode(b"\xff\xfe", None, -3) == ("\ufeff", 2, -3)
    assert utf_16_ex_decode(b"\x00\xdc", "surrogatepass", -1) == ("\udc00", 2, -1)
    class Exporter(bytes):
        def __buffer__(self, flags):
            return memoryview(b"B\x00")

    try:
        result = utf_16_ex_decode(Exporter(b"A\x00"), None, -1)
    except NotImplementedError as exc:
        assert "buffer hooks" in str(exc)
    else:
        assert result == ("B", 2, -1)

    error = UnicodeDecodeError("utf-16-le\x00suffix", b"\x00\xd8", 0, 2, "bad")
    assert codecs.lookup_error("surrogatepass")(error) == ("\ud800", 2)

    def change_input(error):
        assert (error.encoding, error.start, error.end) == ("utf-16-le", 0, 2)
        error.object = b"B\x00C\x00"
        return "?", 0

    codecs.register_error("test.utf16_change_input", change_input)
    wire = b"\x00\xdc"
    assert utf_16_ex_decode(wire, "test.utf16_change_input", -1) == ("?BC", 4, -1)
    assert utf_16_ex_decode(wire, "test.utf16_change_input", -1, True) == ("?BC", 2, -1)


def fixed_endian_streams():
    from io import BytesIO

    for name in ("utf-16-le", "utf-16-be", "utf-32-le", "utf-32-be"):
        entry = codecs.lookup(name)
        module = __import__("encodings." + name.replace("-", "_"), fromlist=["*"])
        assert entry.incrementaldecoder is module.IncrementalDecoder
        text = "\ufeffA🙂"
        wire = text.encode(name)
        stream = BytesIO()
        entry.streamwriter(stream).write(text)
        assert stream.getvalue() == wire
        stream.seek(0)
        assert entry.streamreader(stream).read() == text
        for split in range(len(wire) + 1):
            decoder = entry.incrementaldecoder()
            first = decoder.decode(wire[:split])
            restored = entry.incrementaldecoder()
            restored.setstate(decoder.getstate())
            assert first + restored.decode(wire[split:], True) == text
        primitive = getattr(codecs, name.replace("-", "_") + "_decode")
        assert primitive(wire + b"x") == (text, len(wire))
        assert module.decode(wire + b"x", "replace") == (text + "\ufffd", len(wire) + 1)
