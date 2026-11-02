from json import *

_star_exports = {name for name in globals() if not name.startswith("_")}

import importlib
import json
import sys
from json import encoder


def current():
    expected = ["dump", "dumps", "load", "loads", "JSONDecoder", "JSONDecodeError", "JSONEncoder"]
    assert json.__all__ == expected
    assert _star_exports == set(expected)
    assert encoder is importlib.import_module("json.encoder") is sys.modules["json.encoder"]
    assert JSONEncoder is encoder.JSONEncoder and JSONEncoder.__module__ == "json.encoder"
    assert encoder.py_encode_basestring_ascii('a"\n\U0001d120') == '"a\\"\\n\\ud834\\udd20"'

    original_module = sys.modules.pop("_json")
    try:
        fresh = importlib.import_module("_json")
    finally:
        sys.modules["_json"] = original_module
    assert fresh is not original_module
    for name in ("encode_basestring", "encode_basestring_ascii"):
        quote = getattr(fresh, name)
        assert quote.__self__ is fresh and getattr(quote.__self__, name) is quote
        assert quote is not getattr(original_module, name)
        assert quote("a") == '"a"'

    # The pure escaper still consults the live escape table for code-point strings.
    original_escape = encoder.ESCAPE_DCT["\n"]
    try:
        encoder.ESCAPE_DCT["\n"] = "newline"
        assert encoder.py_encode_basestring_ascii("\ud800\n") == '"\\ud800newline"'
        assert encoder.py_encode_basestring("\ud800\n") == '"\ud800newline"'
    finally:
        encoder.ESCAPE_DCT["\n"] = original_escape

    class Count(int):
        def __int__(self):
            raise AssertionError("unexpected conversion")
        def __repr__(self):
            raise AssertionError("unexpected repr")
    assert list(JSONEncoder().iterencode(Count(-7))) == ["-7"]

    calls = []

    class Encoder(JSONEncoder):
        def default(self, value):
            calls.append(value)
            return 7

    obj = object()
    chunks = Encoder().iterencode([obj])
    assert next(chunks) == "[" and calls == []
    assert list(chunks) == ["7", "]"] and calls == [obj]
    assert json.dumps(obj, cls=Encoder) == "7"

    quote_calls = []
    original_quote = encoder.encode_basestring_ascii
    def quote(text):
        quote_calls.append(text)
        if text == "bad":
            return None
        return "<" + text + ">"
    try:
        encoder.encode_basestring_ascii = quote
        eager = JSONEncoder().iterencode({2: ["red"]}, _one_shot=True)
        assert quote_calls == ["2", "red"] and eager == ("{<2>: [<red>]}",)
        quote_calls.clear()
        JSONEncoder().iterencode([{True: 0, 7: 0}, {True: 0, 7: 0}], _one_shot=True)
        assert quote_calls == ["true", "7", "true", "7"]
        assert quote_calls[0] is quote_calls[2] and quote_calls[1] is not quote_calls[3]
        try:
            JSONEncoder().iterencode(["bad"], _one_shot=True)
        except TypeError as error:
            assert str(error) == "encoder() must return a string, not NoneType"
        else:
            raise AssertionError("invalid quote result")
    finally:
        encoder.encode_basestring_ascii = original_quote

    try:
        json.dumps([], separators=(len, ":"))
    except TypeError as error:
        assert str(error) == "make_encoder() argument 6 must be str, not builtin_function_or_method"
    else:
        raise AssertionError("invalid separator")

    class Factory:
        def __init__(self, **options):
            assert options["separators"] is None and options["skipkeys"]
        def encode(self, value):
            return value

    cached, public = json._default_encoder, json.JSONEncoder
    try:
        json._default_encoder = Encoder()
        json.JSONEncoder = Factory
        assert json.dumps(obj) == "7"
        assert json.dumps(obj, skipkeys=True) is obj
    finally:
        json._default_encoder, json.JSONEncoder = cached, public
    print("json encoder imports, chunks, and dispatch")
