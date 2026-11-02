def current():
    import importlib
    import json
    import sys

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
    # The decoder uses its own globals after the package attribute is rebound.
    fresh.decoder = None
    try:
        fresh.loads('?')
    except fresh.JSONDecodeError as error:
        assert error.doc == '?' and error.pos == 0
    else:
        raise AssertionError('lost decoder error after package mutation')
    print('fresh JSON decoder classes and errors')
