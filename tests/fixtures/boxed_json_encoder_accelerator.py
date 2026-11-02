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


def current():
    make = _json.make_encoder
    assert json.encoder.c_make_encoder is make
    quote = _json.encode_basestring

    def upper(value):
        return quote(value.upper())

    markers = {}
    enc = make(markers=markers, default=str, encoder=upper, indent="..",
               key_separator=": ", item_separator=",", sort_keys=True,
               skipkeys=False, allow_nan=True)
    assert enc.markers is markers and enc.encoder is upper
    row = {"key": [str(i) for i in range(3)], "empty": {}}
    markers[id(row)] = row
    fails(ValueError, enc, row, 0)
    assert markers[id(row)] is row
    markers.clear()
    for level in (-2, 0, 2):
        print(enc(row, level))
    assert not markers
    entries = {"first": 1, "old": 2}

    def replace_key(key):
        if key == "first":
            del entries["old"]
            entries["new"] = 3
        return quote(key)

    changed = make(None, str, replace_key, None, ":", ",", False, False, False)
    print(changed(entries, 0))
    fails(TypeError, enc, row, 1.5)
    fails(TypeError, enc, row)
    fails(TypeError, make, (), str, quote)
    fails(TypeError, make, {}, str, quote, None, None)
    fails(TypeError, make, [], str, quote, None, ": ", ",", False, False, True)

    def broken(value):
        raise LookupError(value)

    for quote_func, error in ((lambda value: 7, TypeError), (broken, LookupError)):
        call = make(None, str, quote_func, None, ": ", ",", False, False, True)
        for value in ("sample", {"entry": 2}):
            fails(error, call, value, 0)

    for index in range(3):
        flags = [False, False, True]
        flags[index] = BadFlag()
        fails(ValueError, make, None, str, quote, None, ": ", ",", *flags)
    for option in ("sort_keys", "skipkeys", "allow_nan", "ensure_ascii", "check_circular"):
        encoder = json.JSONEncoder(**{option: BadFlag()})
        fails(ValueError, encoder.encode, {"x": 4})
    mixed = {"label": "v", 31: False}
    print(json.JSONEncoder(sort_keys=False).encode(mixed))
    fails(TypeError, json.JSONEncoder(sort_keys=True).encode, mixed)
