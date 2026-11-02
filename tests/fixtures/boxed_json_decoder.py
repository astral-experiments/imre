def scanstrings():
    import json
    from _json import scanstring
    assert type(scanstring).__name__ == "builtin_function_or_method"
    assert (scanstring.__module__, scanstring.__qualname__) == ("_json", "scanstring")
    import importlib
    import sys
    accelerator = sys.modules['_json']
    try:
        del sys.modules['_json']
        fresh = importlib.import_module('_json')
        assert fresh is not accelerator
        assert scanstring.__self__ is accelerator and fresh.scanstring.__self__ is fresh
        assert fresh.scanstring is not scanstring
        assert fresh.scanstring('"new"', 1) == ('new', 5)
        assert scanstring('"old"', 1) == ('old', 5)
    finally:
        sys.modules['_json'] = accelerator
    assert json.decoder.scanstring is scanstring
    assert scanstring('é"\\ud800\\udc00x"!', 2) == (chr(0x10000) + 'x', 16)
    assert scanstring('"a\nb"', 1, False) == ('a\nb', 5)
    unpaired = '"' + chr(0xd800) + '\\udc00"'
    assert scanstring(unpaired, 1) == (chr(0xd800) + chr(0xdc00), len(unpaired))
    malformed = 'é"\\x"'
    try:
        scanstring(malformed, 2)
    except json.JSONDecodeError as error:
        assert error.doc is malformed and error.pos == 2 and error.msg == 'Invalid \\escape'
    else:
        raise AssertionError('accepted invalid escape')


def surrogate_documents():
    import json

    high, low = chr(0xd83d), chr(0xdc0d)
    assert json.loads('"'+high+low+'"') == high+low
    assert json.loads(r'"\ud83d\udc0d"') == chr(0x1f40d)
    assert json.loads('"'+high+r'\udc0d"') == high+low
    assert json.loads(r'"\ud83d'+low+'"') == high+low
    assert json.loads(br'"\ud83d"') == high
    doc = '[{"'+high+'":1},{"\\ud83d":2,"'+high+'":3},{"\\u0061":"ø"}]'
    a, b, c = json.loads(doc)
    assert (a, b, c) == ({high:1}, {high:3}, {'a':'ø'})
    assert next(iter(a)) is next(iter(b))
    bom = '\ufeff"'+high+'"'
    try:
        json.loads(bom)
    except json.JSONDecodeError as error:
        assert error.doc is bom and (error.pos, error.lineno, error.colno) == (0, 1, 1)
        assert error.msg == 'Unexpected UTF-8 BOM (decode using utf-8-sig)'
    else:
        raise AssertionError('leading BOM was accepted')
    prefix = '["'+high+'😀",\n'
    malformed = prefix+'?]'
    try:
        json.loads(malformed)
    except json.JSONDecodeError as error:
        assert error.doc is malformed and error.msg == 'Expecting value'
        assert (error.pos, error.lineno, error.colno) == (len(prefix),2,1)
    else:
        raise AssertionError('invalid value was accepted')


def cached_decoder(package):
    cache = package._default_decoder
    calls = []
    marker = object()

    def decode(doc):
        assert type(doc) is str
        calls.append(doc)
        return marker

    cache.decode = decode
    try:
        text = '"é"'
        assert package.loads(text) is marker
        assert package.loads(text.encode()) is marker
        assert package.loads(bytearray(b'\xfe\xff\x00"\x00\xe9\x00"')) is marker
        assert calls == [text, text, text] and calls[0] is text
    finally:
        del cache.decode


def current():
    import importlib
    import json
    import sys

    scanstrings()
    surrogate_documents()
    names = ('json', 'json.decoder', 'json.scanner', 'json.encoder', '_json')
    saved = {name: sys.modules[name] for name in names if name in sys.modules}
    try:
        for name in saved:
            del sys.modules[name]
        sys.modules['_json'] = None
        fresh = importlib.import_module('json')
    finally:
        for name in names:
            sys.modules.pop(name, None)
        sys.modules.update(saved)

    assert fresh is not json and fresh.JSONDecodeError is not json.JSONDecodeError
    assert fresh.scanner.make_scanner.__module__ == 'json.scanner'
    assert fresh.decoder.scanstring.__module__ == 'json.decoder'
    for package in (json, fresh):
        cached_decoder(package)
        assert package.JSONDecoder is package.decoder.JSONDecoder
        assert package.JSONDecodeError is package.decoder.JSONDecodeError
        assert package.JSONDecoder().decode('["é", 2]') == ['é', 2]
        assert package.loads(s='[null]') == [None]
        document = '{\n "é":]}'
        for source in (document, document.encode()):
            try:
                package.loads(source)
            except package.JSONDecodeError as error:
                assert type(error) is package.JSONDecodeError
                assert error.doc == document and (type(source) is bytes or error.doc is source)
                assert (error.msg, error.pos, error.lineno, error.colno) == ('Expecting value', 7, 2, 6)
                assert error.args == ('Expecting value: line 2 column 6 (char 7)',)
            else:
                raise AssertionError('accepted malformed JSON')
    document = '"' + chr(0xd800) + '\\q"'
    try:
        fresh.loads(document)
    except fresh.JSONDecodeError as error:
        assert error.doc is document and error.pos == 3
        assert error.msg == "Invalid \\escape: 'q'"
    else:
        raise AssertionError('accepted invalid escape after surrogate')
    # The decoder uses its own globals after the package attribute is rebound.
    fresh.decoder = None
    try:
        fresh.loads('?')
    except fresh.JSONDecodeError as error:
        assert error.doc == '?' and error.pos == 0
    else:
        raise AssertionError('lost decoder error after package mutation')
    print('fresh JSON decoder classes and errors')

    class Source:
        def __init__(self):
            self.calls = 0

        def read(self):
            self.calls += 1
            return b'{"key": 1, "key": 2.00}'

    def rejected(item):
        raise AssertionError("object_hook must yield to object_pairs_hook")

    stream = Source()
    result = json.load(stream, parse_float=str, parse_int=str,
                       object_pairs_hook=list, object_hook=rejected)
    assert result == [('key', '1'), ('key', '2.00')]
    assert stream.calls == 1

    class Decoder:
        def __init__(self, *, label):
            self.label = label

        def decode(self, text):
            return self.label, text

    assert json.loads(b'123', cls=Decoder, label='document') == ('document', '123')

    class Text(str):
        pass

    fresh.JSONDecoder = None
    assert fresh.loads(Text('[3]')) == [3]
    assert fresh.loads(bytearray(b'\xff\xfe1\x00')) == 1
