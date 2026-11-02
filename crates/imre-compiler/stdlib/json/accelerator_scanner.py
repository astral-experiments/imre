from _json import scanstring as _scanner_scanstring
from _imre_json import scan_once as _native_scan_once


def _scanner_arguments(function, names, args, kwargs):
    limit = len(names)
    if not kwargs and len(args) == limit:
        return args
    count = len(args) + len(kwargs)
    if count > limit:
        kind = "keyword " if not args else ""
        plural = "" if limit == 1 else "s"
        raise TypeError(f"{function}() takes at most {limit} {kind}argument{plural} ({count} given)")
    values = list(args)
    for i in range(len(args), limit):
        name = names[i]
        if name not in kwargs:
            raise TypeError(f"{function}() missing required argument '{name}' (pos {i + 1})")
        values.append(kwargs[name])
    return values


def _readonly_scanner_option(index):
    def refuse(*args):
        raise AttributeError("readonly attribute")
    return property(lambda scanner: scanner._options[index], refuse, refuse)


class Scanner:
    """JSON scanner object"""

    __slots__ = ("_options", "_scan")
    strict = _readonly_scanner_option(0)
    object_hook = _readonly_scanner_option(1)
    object_pairs_hook = _readonly_scanner_option(2)
    parse_float = _readonly_scanner_option(3)
    parse_int = _readonly_scanner_option(4)
    parse_constant = _readonly_scanner_option(5)

    def __new__(cls, /, *args, **kwargs):
        context, = _scanner_arguments("make_scanner", ("context",), args, kwargs)
        self = object.__new__(cls)
        options = (bool(context.strict), context.object_hook, context.object_pairs_hook,
                   context.parse_float, context.parse_int, context.parse_constant)
        object.__setattr__(self, "_options", options)

        # Capture the parsers after validating context, including when _json was
        # imported directly before json. The C scanner ignores the context's parsers.
        from functools import partial
        from json import decoder
        constants = getattr(decoder, "_CONSTANTS", None)
        constant = self.parse_constant
        if (self.strict and self.object_hook is None and self.object_pairs_hook is None
                and self.parse_float is float and self.parse_int is int
                and type(constants) is dict and type(constant) is type(constants.__getitem__)
                and constant.__name__ == '__getitem__' and constant.__self__ is constants):
            scan = partial(_native_scan_once, constant)
        else:
            from json.scanner import _make_scan_once, NUMBER_RE
            parse_object = partial(decoder.JSONObject, _propagate_stop=True, _key_scanstring=_scanner_scanstring)
            parse_array = partial(decoder.JSONArray, _propagate_stop=True)
            parser = partial(_make_scan_once, parse_object, parse_array, _scanner_scanstring, NUMBER_RE.match,
                             self.strict, self.parse_float, self.parse_int, constant,
                             self.object_hook, self.object_pairs_hook)

            def scan(string, idx):
                # Reentrant calls from hooks must intern keys in their own memo.
                return parser({})(string, idx)

        object.__setattr__(self, "_scan", scan)
        return self

    def __init__(self, /, *args, **kwargs):
        # The native type only initializes state in __new__.
        pass

    def __init_subclass__(cls, **kwargs):
        raise TypeError("type '_json.Scanner' is not an acceptable base type")

    def __setattr__(self, name, value):
        self.__delattr__(name)

    def __delattr__(self, name):
        if name in ("strict", "object_hook", "object_pairs_hook", "parse_float", "parse_int", "parse_constant"):
            raise AttributeError("readonly attribute")
        raise AttributeError(f"'_json.Scanner' object has no attribute '{name}' and no __dict__ for setting new attributes")

    def __call__(self, /, *args, **kwargs):
        from operator import index
        from sys import maxsize

        string, idx = _scanner_arguments("scan_once", ("string", "idx"), args, kwargs)
        idx = index(idx)
        if not -maxsize - 1 <= idx <= maxsize:
            raise OverflowError("Python int too large to convert to C ssize_t")
        if not isinstance(string, str):
            raise TypeError(f"first argument must be a string, not {type(string).__name__[:80]}")
        if idx < 0:
            raise ValueError("idx cannot be negative")

        scan = self._scan
        if type(string) is str:
            return scan(string, idx)

        # Like the C parser, read the underlying text without invoking overrides.
        from json.decoder import JSONDecodeError
        text = str.__str__(string)
        try:
            return scan(text, idx)
        except JSONDecodeError as error:
            if error.doc is text:
                error.doc = string
            raise


make_scanner = Scanner
del Scanner
