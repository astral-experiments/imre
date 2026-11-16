import _codecs
import codecs
import encodings
import io
import sys


def decode(value):
    if isinstance(value, memoryview):
        assert value.obj is None and value.readonly
        views.append(bytes(value))
    return bytes(value)[::-1].decode(), len(value)


def search(name):
    calls.append(name)
    if enabled and name == "user_test":
        return codecs.CodecInfo(
            name=name,
            encode=lambda value: (value[::-1].encode(), len(value)),
            decode=decode,
        )


def miss(name):
    calls.append("miss:" + name)


def custom_escape(name):
    if name == "raw_unicode_escape":
        calls.append(name)
        return (lambda value: (b"custom", len(value)), lambda value: ("custom", len(value)), None, None)


def decode_live(view):
    assert view.readonly and view.obj is None
    source[0] = 120
    assert view[::2].tobytes() == b"xc"
    retained.append(view)
    return ("ok", len(view))


def live_codec(name):
    if name == "live_view":
        return (None, decode_live, None, None)


assert codecs.register is _codecs.register
assert codecs.unregister is _codecs.unregister
assert codecs.lookup is _codecs.lookup

for alias, name, expected in (("US-ASCII", "ascii", b"A??"), ("L1", "latin-1", b"A\xe9?")):
    encoder = codecs.getincrementalencoder(alias)
    assert encoder is codecs.lookup(name).incrementalencoder
    assert encoder("replace").encode("Aé☃", final=True) == expected
    assert encoder().encode("", final=True) == b""

writer_class = codecs.getwriter(encoding="windows-1251")
assert writer_class is codecs.lookup("cp1251").streamwriter
stream = io.BytesIO()
writer = writer_class(stream)
writer.write("до свидания\n")
expected = b"\xe4\xee \xf1\xe2\xe8\xe4\xe0\xed\xe8\xff\n"
assert stream.getvalue() == expected
try:
    writer.write("x☃")
except UnicodeEncodeError as error:
    assert (error.object, error.start, error.end) == ("x☃", 1, 2)
    assert stream.getvalue() == expected
else:
    raise AssertionError("strict writer accepted an unencodable character")


def replace_snowman(error):
    assert (error.object, error.start, error.end) == ("ё☃!", 1, 2)
    return b"<snow>", error.end


codecs.register_error("boxed_writer_snowman", replace_snowman)
writer.errors = "boxed_writer_snowman"
writer.write("ё☃!")
assert stream.getvalue() == expected + b"\xb8<snow>!"


for encoding in ("ascii", "utf-8", "utf-16-be", "cp1252"):
    entry = codecs.lookup(encoding)
    assert codecs.getencoder(encoding) is entry.encode
    assert codecs.getdecoder(encoding) is entry.decode
    encoded, used = codecs.getencoder(encoding)("A")
    assert used == 1 and codecs.getdecoder(encoding)(encoded) == ("A", len(encoded))


calls = []
views = []
enabled = False

codecs.register(miss)
codecs.register(search)
try:
    codecs.lookup("utf-8")
    assert calls == []
    try:
        codecs.lookup("User-Test")
    except LookupError:
        pass
    else:
        raise AssertionError("disabled codec was found")

    enabled = True
    info = codecs.lookup("USER test")
    assert calls == ["miss:user_test", "user_test"] * 2
    assert info.name == "user_test"
    assert codecs.getwriter("user-test") is None
    info.streamwriter = writer_class
    assert codecs.getwriter("user-test") is writer_class
    assert codecs.lookup("user-test") is info
    assert codecs.encode("abc", "User-Test") == b"cba"
    assert codecs.decode(b"cba", "user test") == "abc"
    assert b"cba".decode("user test") == "abc"
    assert views == [b"cba"]
    assert len(calls) == 4

    codecs.unregister(lambda name: None)
    assert codecs.lookup("user_test") is info
    codecs.unregister(miss)
    assert codecs.lookup("user_test") is not info
    assert calls[-1] == "user_test"
finally:
    codecs.unregister(miss)
    codecs.unregister(search)

try:
    codecs.lookup("user-test")
except LookupError:
    pass
else:
    raise AssertionError("unregistered codec was still cached")


calls.clear()
codecs.register(custom_escape)
try:
    assert codecs.decode(br"\u0042", "raw-unicode-escape") == "B"
    assert "B".encode("raw-unicode-escape") == b"B"
    assert calls == []
    codecs.unregister(encodings.search_function)
    try:
        assert codecs.decode(br"\u0042", "raw-unicode-escape") == "custom"
        assert "B".encode("raw-unicode-escape") == b"custom"
        assert codecs.encode("B", "raw-unicode-escape") == b"custom"
        assert codecs.lookup("raw_unicode_escape")[1](b"x") == ("custom", 1)
        assert calls == ["raw_unicode_escape"]
    finally:
        codecs.register(encodings.search_function)
finally:
    codecs.unregister(custom_escape)

assert codecs.decode(br"\u0042", "raw-unicode-escape") == "B"

source = bytearray(b"abc")
retained = []
codecs.register(live_codec)
try:
    assert source.decode("live-view") == "ok"
    source[1] = 121
    assert retained[0].tobytes() == b"xyc"
    source.append(122)
finally:
    retained.clear()
    codecs.unregister(live_codec)


for name in ('utf-8-sig', 'utf-16', 'utf-32'):
    info = codecs.lookup(name)
    assert info.name == name
    prefix = ''.encode(name)
    assert info.encode('x') == ('x'.encode(name), 1)
    encoder = info.incrementalencoder()
    first = encoder.getstate()
    assert encoder.encode('') == prefix
    assert encoder.getstate() == 0
    payload = encoder.encode('x', final=True)
    assert prefix + payload == 'x'.encode(name)
    encoder.reset()
    assert encoder.getstate() == first
    encoder.setstate(0)
    assert encoder.encode('x') == payload
    decoder = info.incrementaldecoder()
    assert decoder.decode(prefix[:1]) == ''
    partial = decoder.getstate()
    decoder.reset()
    decoder.setstate(partial)
    assert decoder.decode(prefix[1:] + payload, final=True) == 'x'
    assert info.decode(bytearray(prefix + payload)) == ('x', len(prefix + payload))


wide_bytes = encodings.search_function(b'utf-16')
assert wide_bytes is encodings.search_function(b'utf-16')
assert wide_bytes is not encodings.search_function('utf-16')
for length in range(1, 502):
    encodings.search_function('utf' + '-' * length + '32')
assert encodings.search_function(b'utf-16') is not wide_bytes


for width, encode in ((2, codecs.utf_16_encode), (4, codecs.utf_32_encode)):
    for order, endian in ((-2, 'little'), (2, 'big')):
        # UTF-32 names noncanonical byte orders with its generic encoding.
        name = 'utf-32' if width == 4 else ('utf-16-le' if order < 0 else 'utf-16-be')
        text = 'A\ud800'
        try:
            encode(text, 'strict', order)
        except UnicodeEncodeError as error:
            assert error.object is text
            assert error.args == (name, text, 1, 2, 'surrogates not allowed')
        else:
            raise AssertionError('strict encoder accepted an unpaired surrogate')
        prefix = ord('A').to_bytes(width, endian)
        replacement = (0xd800).to_bytes(width, sys.byteorder if width == 4 else endian)
        assert encode(text, 'surrogatepass', order) == (prefix + replacement, 2)


class FallbackReader:
    def __init__(self, stream, errors='strict'):
        self.stream = stream
        self.errors = errors

    def read(self):
        return self.stream.read().decode('utf-16', self.errors)


def app_reader(name):
    if name == 'app_wide_reader':
        return encodings.search_function('utf_16') or codecs.CodecInfo(
            None, None, streamreader=FallbackReader, name=name)


codecs.register(app_reader)
try:
    info = codecs.lookup('app-wide-reader')
    assert codecs.lookup('app_wide_reader') is info
    assert info.streamreader(io.BytesIO('é'.encode('utf-16'))).read() == 'é'
finally:
    codecs.unregister(app_reader)

for alias, codec, bom in (('u16', 'utf-16-be', codecs.BOM_UTF16_BE),
                          ('u32', 'utf-32-be', codecs.BOM_UTF32_BE)):
    factory = codecs.getreader(alias)
    assert factory is codecs.lookup(alias).streamreader
    reader = factory(io.BytesIO(bom + 'Aé\nΩ'.encode(codec)))
    assert reader.read(1, 1) == 'A'
    assert reader.readline() == 'é\n'
    assert reader.read() == 'Ω'
    reader.seek(0)
    assert reader.read() == 'Aé\nΩ'
    try:
        factory(io.BytesIO(bytes(8))).read()
    except UnicodeDecodeError as error:
        assert error.reason == 'Stream does not start with BOM'
    else:
        raise AssertionError('stream reader accepted wide text without BOM')
