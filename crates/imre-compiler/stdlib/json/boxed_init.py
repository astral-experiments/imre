# decoder.py, encoder.py and scanner.py are unmodified CPython 3.14.5 sources at
# 5607950ef232dad16d75c0cf53101d9649d89115 (see licenses/CPython.txt).
from . import decoder
from .decoder import JSONDecoder, JSONDecodeError
from .encoder import JSONEncoder
from _imre_json import loads as _native_loads


def _raise_decode_error(message, document, position, bom, _decoder=decoder):
    cls = JSONDecodeError if bom else _decoder.JSONDecodeError
    raise cls(message, document, position)


def loads(s):
    return _native_loads(s, _raise_decode_error)
