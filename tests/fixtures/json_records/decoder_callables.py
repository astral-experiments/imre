import json
from typing import Any, Callable


def decode_keys(read: Callable[[str], list[dict[str, Any]]], source: str) -> list[str]:
    rows = read(source)
    keys = sorted(rows[0])
    other = sorted(rows[1])
    print(keys[0] is other[0])
    print(keys[1] is other[1])
    return keys


class Reader:
    def load(self, text: str) -> list[dict[str, Any]]:
        return json.loads(text)


sample = r'[{"b_é":0,"a_key":1},{"\u0061_key":2,"b_\u00e9":3}]'
print(decode_keys(json.loads, sample))
print(decode_keys(Reader().load, sample))
decoder = json.JSONDecoder()
saved: Callable[[str], list[dict[str, Any]]] = decoder.decode
decoder = json.JSONDecoder()
print(decode_keys(saved, sample))
print(decode_keys(decoder.decode, sample))
load_int: Callable[[bytes], int] = json.loads
print(load_int(b"123456789012345678901234567890"))
read_string: Callable[[str], str] = json.JSONDecoder().decode
print(read_string('"escaped\\u00e9"'))
plain = json.loads
bound = decoder.decode
print(repr(plain('false')))
print(repr(bound('[1,2]')))
try:
    plain('\ufeff{}')
except json.JSONDecodeError as error:
    print(error.msg)
    print(error.pos)
try:
    bound('\ufeff{}')
except json.JSONDecodeError as error:
    print(error.msg)
    print(error.pos)
