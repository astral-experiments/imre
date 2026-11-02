# decoder.py and scanner.py are unmodified CPython 3.14.5 sources at
# 5607950ef232dad16d75c0cf53101d9649d89115 (see licenses/CPython.txt).
from . import decoder
from .decoder import JSONDecoder, JSONDecodeError
from _imre_json import loads as _native_loads

_default_decoder = JSONDecoder()


def _raise_decode_error(message, document, position, bom, _decoder=decoder):
    cls = JSONDecodeError if bom else _decoder.JSONDecodeError
    raise cls(message, document, position)


def detect_encoding(b):
    bstartswith = b.startswith
    if bstartswith((b'\x00\x00\xfe\xff', b'\xff\xfe\x00\x00')):
        return 'utf-32'
    if bstartswith((b'\xfe\xff', b'\xff\xfe')):
        return 'utf-16'
    if bstartswith(b'\xef\xbb\xbf'):
        return 'utf-8-sig'

    if len(b) >= 4:
        if not b[0]:
            # 00 00 -- -- - utf-32-be
            # 00 XX -- -- - utf-16-be
            return 'utf-16-be' if b[1] else 'utf-32-be'
        if not b[1]:
            # XX 00 00 00 - utf-32-le
            # XX 00 00 XX - utf-16-le
            # XX 00 XX -- - utf-16-le
            return 'utf-16-le' if b[2] or b[3] else 'utf-32-le'
    elif len(b) == 2:
        if not b[0]:
            # 00 XX - utf-16-be
            return 'utf-16-be'
        if not b[1]:
            # XX 00 - utf-16-le
            return 'utf-16-le'
    # default
    return 'utf-8'


def load(fp, *, cls=None, object_hook=None, parse_float=None,
        parse_int=None, parse_constant=None, object_pairs_hook=None, **kw):
    return loads(fp.read(),
        cls=cls, object_hook=object_hook,
        parse_float=parse_float, parse_int=parse_int,
        parse_constant=parse_constant, object_pairs_hook=object_pairs_hook, **kw)


def loads(s, *, cls=None, object_hook=None, parse_float=None,
        parse_int=None, parse_constant=None, object_pairs_hook=None, **kw):
    use_default = (cls is None and object_hook is None and parse_int is None and
                   parse_float is None and parse_constant is None and object_pairs_hook is None and not kw)
    if use_default:
        if type(s) is bytearray:
            s = bytes(s)
        if type(s) is str or type(s) is bytes:
            return _native_loads(s, _raise_decode_error)
    if isinstance(s, str):
        if s.startswith('\ufeff'):
            raise JSONDecodeError("Unexpected UTF-8 BOM (decode using utf-8-sig)",
                                  s, 0)
    else:
        if not isinstance(s, (bytes, bytearray)):
            raise TypeError(f'the JSON object must be str, bytes or bytearray, '
                            f'not {s.__class__.__name__}')
        s = s.decode(detect_encoding(s), 'surrogatepass')

    if use_default:
        return _default_decoder.decode(s)
    if cls is None:
        cls = JSONDecoder
    if object_hook is not None:
        kw['object_hook'] = object_hook
    if object_pairs_hook is not None:
        kw['object_pairs_hook'] = object_pairs_hook
    if parse_float is not None:
        kw['parse_float'] = parse_float
    if parse_int is not None:
        kw['parse_int'] = parse_int
    if parse_constant is not None:
        kw['parse_constant'] = parse_constant
    return cls(**kw).decode(s)
