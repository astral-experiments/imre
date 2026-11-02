from _json import scanstring as _scanner_scanstring


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


class Scanner:
    """JSON scanner object"""

    __slots__ = ("strict", "object_hook", "object_pairs_hook",
                 "parse_float", "parse_int", "parse_constant", "_scan")

    def __new__(cls, /, *args, **kwargs):
        context, = _scanner_arguments("make_scanner", ("context",), args, kwargs)
        self = object.__new__(cls)
        object.__setattr__(self, "strict", bool(context.strict))
        for name in ("object_hook", "object_pairs_hook", "parse_float", "parse_int", "parse_constant"):
            object.__setattr__(self, name, getattr(context, name))

        # Capture the parsers after validating context, including when _json was
        # imported directly before json. The C scanner ignores the context's parsers.
        from functools import partial
        from json.decoder import JSONArray, JSONObject
        from json.scanner import _make_scan_once, NUMBER_RE
        parse_object = partial(JSONObject, _propagate_stop=True, _key_scanstring=_scanner_scanstring)
        parse_array = partial(JSONArray, _propagate_stop=True)
        parser = partial(_make_scan_once, parse_object, parse_array, _scanner_scanstring, NUMBER_RE.match,
                         self.strict, self.parse_float, self.parse_int, self.parse_constant,
                         self.object_hook, self.object_pairs_hook)
        object.__setattr__(self, "_scan", parser)
        return self

    def __init_subclass__(cls, **kwargs):
        raise TypeError("type '_json.Scanner' is not an acceptable base type")

    def __setattr__(self, name, value):
        raise AttributeError("readonly attribute")

    def __delattr__(self, name):
        raise AttributeError("readonly attribute")

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

        # Reentrant calls from hooks must intern keys in their own memo.
        scan = self._scan({})
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
