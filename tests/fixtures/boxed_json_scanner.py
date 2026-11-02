import _json
import json


def fails(error_type, func, *args, **kwargs):
    try:
        func(*args, **kwargs)
    except error_type as error:
        print(type(error).__name__, str(error))
    else:
        raise AssertionError("expected " + error_type.__name__)


class BadFlag:
    def __bool__(self):
        raise ValueError("could not convert flag")


class Context:
    def __init__(self):
        self.reads = []
        self.options = dict(strict=False, object_hook=None, object_pairs_hook=list,
                            parse_float=str, parse_int=str, parse_constant=str)

    def __getattr__(self, name):
        self.reads.append(name)
        return self.options[name]


def current():
    make = _json.make_scanner
    assert json.scanner.make_scanner is json.scanner.c_make_scanner is make
    default_scan = json.JSONDecoder().scan_once
    document = 'é [NaN, NaN]  suffix'
    values, end = default_scan(document, 2)
    assert end == document.index(']') + 1
    assert values[0] is values[1] is json.decoder._CONSTANTS['NaN']
    assert json.loads('[NaN, Infinity, -Infinity]', parse_constant=str.upper) == [
        'NAN', 'INFINITY', '-INFINITY'
    ]

    class ConstantHook:
        def __getattribute__(self, name):
            if name in ('__class__', '__name__', '__self__'):
                raise AssertionError(name)
            return object.__getattribute__(self, name)

        def __call__(self, name):
            raise LookupError(name)

    hooked = json.JSONDecoder(parse_constant=ConstantHook())
    assert hooked.decode('42') == 42
    fails(LookupError, hooked.decode, 'NaN')
    assert json.loads('NaN', parse_constant={'NaN': 17}.__getitem__) == 17
    constants = json.decoder._CONSTANTS
    del json.decoder._CONSTANTS
    try:
        assert json.JSONDecoder(parse_constant=str).decode("NaN") == "NaN"
    finally:
        json.decoder._CONSTANTS = constants
    decoder = json.decoder
    json.decoder = None
    try:
        assert json.JSONDecoder(parse_constant=str).decode("NaN") == "NaN"
    finally:
        json.decoder = decoder
    try:
        default_scan('é [ ?', 2)
    except StopIteration as error:
        assert error.value == 4
    else:
        raise AssertionError('scanner accepted missing array value')
    context = Context()
    scan = make(context=context)
    print(context.reads)
    context.options["parse_int"] = int
    document = 'tag{"item":[31,4.50,NaN],"text":"x\ty"}rest'
    print(scan(string=document, idx=document.index("{")))
    assert scan.__init__(object()) is None
    print(scan("v82!", 1))
    fails(TypeError, type, "DerivedScanner", (make,), {})
    fails(StopIteration, scan, '{"bad":[?]}', 0)
    fails(AttributeError, make, 5)
    fails(ValueError, json.JSONDecoder, strict=BadFlag())
    fails(ValueError, scan, "12", -1)
    fails(StopIteration, scan, "12", 8)
    fails(TypeError, scan, [], 0)
