from _imre_csv import read_row as _read_row
from sys import maxsize as _maxsize


_field_limit = 131072
_no_limit_argument = object()


def field_size_limit(new_limit=_no_limit_argument):
    global _field_limit
    previous = _field_limit
    if new_limit is not _no_limit_argument:
        if type(new_limit) is not int:
            raise TypeError("limit must be an integer")
        if new_limit > _maxsize or new_limit < -_maxsize - 1:
            raise OverflowError("Python int too large to convert to C ssize_t")
        _field_limit = new_limit
    return previous


class _Reader:
    def __init__(self, iterator, dialect):
        self._iterator = iterator
        self._dialect = dialect
        self._line_num = 0
        self._generation = 0

    @property
    def dialect(self):
        return self._dialect

    @property
    def line_num(self):
        return self._line_num

    def __iter__(self):
        return self

    def _next_line(self, generation):
        line = next(self._iterator)
        if not isinstance(line, str):
            raise Error("iterator should return strings, not " + type(line).__name__ +
                        " (the file should be opened in text mode)")
        if self._generation != generation:
            raise Error("iterator has already advanced the reader")
        self._line_num += 1
        return line, _field_limit

    def __next__(self):
        d = self._dialect
        flags = int(d.doublequote) | (int(d.skipinitialspace) << 1) | (int(d.strict) << 2)
        quote = None if d.quoting == QUOTE_NONE else d.quotechar
        result = _read_row(self._next_line, self._generation, d.delimiter, quote,
                           d.escapechar, flags, d.quoting, Error)
        self._generation += 1
        return result


def reader(iterable, /, dialect=None, **fmtparams):
    return _Reader(iter(iterable), _dialect(dialect, **fmtparams))
