import json
import sys
from json.encoder import JSONEncoder


def current():
    assert json.encoder is sys.modules["json.encoder"]
    assert JSONEncoder is json.JSONEncoder
    # The original example on the public encode method.
    assert JSONEncoder().encode({"foo": ["bar", "baz"]}) == '{"foo": ["bar", "baz"]}'
    encoder = JSONEncoder(default=lambda obj: [obj.real, obj.imag])
    assert "".join(encoder.iterencode(2 + 1j)) == "[2.0, 1.0]"
    print("JSONEncoder module, class and methods verified")
