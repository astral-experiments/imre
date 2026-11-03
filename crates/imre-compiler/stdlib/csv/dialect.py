"""Dialect resolution for runtime-loaded CSV readers and writers."""

__all__ = ["QUOTE_MINIMAL", "QUOTE_ALL", "QUOTE_NONNUMERIC", "QUOTE_NONE",
           "QUOTE_STRINGS", "QUOTE_NOTNULL", "Error", "Dialect", "excel", "excel_tab",
           "field_size_limit", "reader", "writer", "register_dialect", "get_dialect",
           "list_dialects", "Sniffer", "unregister_dialect", "DictReader", "DictWriter",
           "unix_dialect"]
__version__ = "1.0"

QUOTE_MINIMAL = 0
QUOTE_ALL = 1
QUOTE_NONNUMERIC = 2
QUOTE_NONE = 3
QUOTE_STRINGS = 4
QUOTE_NOTNULL = 5


class Error(Exception):
    pass


class _Format:
    """The registered format is a read-only snapshot of its source dialect."""

    def __setattr__(self, name, value):
        raise AttributeError("readonly attribute")

    def __delattr__(self, name):
        raise AttributeError("readonly attribute")

    def __reduce_ex__(self, protocol):
        raise TypeError("cannot pickle '_csv.Dialect' instances")


_defaults = {
    "delimiter": ",", "doublequote": True, "escapechar": None,
    "lineterminator": "\r\n", "quotechar": '"', "quoting": QUOTE_MINIMAL,
    "skipinitialspace": False, "strict": False,
}
_registry = {}


def _character(name, value):
    optional = name != "delimiter"
    if value is None and optional:
        return value
    if isinstance(value, str):
        # Character options store a Unicode scalar, even when passed a str subclass.
        value = "".join((value,))
        if len(value) == 1:
            return value
        actual = "a string of length " + str(len(value))
    else:
        actual = type(value).__name__
    expected = "a unicode character or None" if optional else "a unicode character"
    raise TypeError('"' + name + '" must be ' + expected + ", not " + actual)


def _dialect(dialect=None, **options):
    if isinstance(dialect, str):
        dialect = get_dialect(dialect)
    if isinstance(dialect, _Format) and not options:
        return dialect
    for name in options:
        if name not in _defaults:
            raise TypeError("'" + name + "' is an invalid keyword argument for this function")
    for name in _defaults:
        if name not in options and dialect is not None:
            try:
                options[name] = getattr(dialect, name)
            except BaseException:
                # CPython treats an attribute lookup failure on the dialect as an omitted option.
                pass
    if "quoting" not in options and options.get("quotechar", '"') is None:
        options["quoting"] = QUOTE_NONE
    result = _Format()
    for name in _defaults:
        value = options.get(name, _defaults[name])
        if name in ("delimiter", "escapechar", "quotechar"):
            value = _character(name, value)
        elif name == "lineterminator":
            if not isinstance(value, str):
                raise TypeError('"lineterminator" must be a string, not ' + type(value).__name__)
        elif name == "quoting":
            if type(value) is not int:
                raise TypeError('"quoting" must be an integer, not ' + type(value).__name__)
            if value < -2147483648 or value > 2147483647:
                raise OverflowError("Python int too large to convert to C int")
            if value < 0 or value > QUOTE_NOTNULL:
                raise TypeError('bad "quoting" value')
        else:
            value = bool(value)
        object.__setattr__(result, name, value)
    if result.quotechar is None and result.quoting != QUOTE_NONE:
        raise TypeError("quotechar must be set if quoting enabled")
    terminator = "".join((result.lineterminator,))
    names = ("delimiter", "escapechar", "quotechar")
    for name in names:
        char = getattr(result, name)
        if char is None:
            continue
        if char in "\r\n" or (char == " " and name != "delimiter" and result.skipinitialspace):
            raise ValueError("bad " + name + " value")
        if char in terminator:
            raise ValueError("bad " + name + " or lineterminator value")
    for first, second in (("delimiter", "escapechar"), ("delimiter", "quotechar"),
                          ("escapechar", "quotechar")):
        char = getattr(result, first)
        if char is not None and char == getattr(result, second):
            raise ValueError("bad " + first + " or " + second + " value")
    return result


def register_dialect(name, /, dialect=None, **fmtparams):
    if not isinstance(name, str):
        raise TypeError("dialect name must be a string")
    _registry[name] = _dialect(dialect, **fmtparams)


def list_dialects():
    return list(_registry)


def get_dialect(name):
    try:
        return _registry[name]
    except KeyError:
        raise Error("unknown dialect") from None


def unregister_dialect(name):
    try:
        del _registry[name]
    except KeyError:
        raise Error("unknown dialect") from None


# Public dialect classes and registration from CPython 3.14.5 Lib/csv.py.
class Dialect:
    """Describe a CSV dialect.

    This must be subclassed (see csv.excel).  Valid attributes are:
    delimiter, quotechar, escapechar, doublequote, skipinitialspace,
    lineterminator, quoting.

    """
    _name = ""
    _valid = False
    # placeholders
    delimiter = None
    quotechar = None
    escapechar = None
    doublequote = None
    skipinitialspace = None
    lineterminator = None
    quoting = None

    def __init__(self):
        if self.__class__ != Dialect:
            self._valid = True
        self._validate()

    def _validate(self):
        try:
            _dialect(self)
        except TypeError as e:
            # Re-raise to get a traceback showing more user code.
            raise Error(str(e)) from None

class excel(Dialect):
    """Describe the usual properties of Excel-generated CSV files."""
    delimiter = ','
    quotechar = '"'
    doublequote = True
    skipinitialspace = False
    lineterminator = '\r\n'
    quoting = QUOTE_MINIMAL
register_dialect("excel", excel)

class excel_tab(excel):
    """Describe the usual properties of Excel-generated TAB-delimited files."""
    delimiter = '\t'
register_dialect("excel-tab", excel_tab)

class unix_dialect(Dialect):
    """Describe the usual properties of Unix-generated CSV files."""
    delimiter = ','
    quotechar = '"'
    doublequote = True
    skipinitialspace = False
    lineterminator = '\n'
    quoting = QUOTE_ALL
register_dialect("unix", unix_dialect)
