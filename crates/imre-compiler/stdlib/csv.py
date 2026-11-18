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

class _Writer:
    def __init__(self, write, dialect):
        self._write = write
        self._dialect = dialect

    @property
    def dialect(self):
        return self._dialect

    def writerow(self, row, /):
        try:
            fields = iter(row)
        except TypeError:
            raise Error("iterable expected, not " + type(row).__name__) from None

        d = self._dialect
        mode = d.quoting
        terminator = "".join((d.lineterminator,))
        syntax = d.delimiter + "\r\n" + terminator
        if mode == QUOTE_NONE:
            syntax += (d.quotechar or "") + (d.escapechar or "")
        record = []
        while True:
            try:
                field = next(fields)
            except StopIteration:
                break
            quoted = mode == QUOTE_ALL
            if mode == QUOTE_NONNUMERIC:
                # PyNumber_Check uses the conversion slots, including on user types.
                cls = type(field)
                quoted = not (isinstance(field, (int, float, complex))
                              or hasattr(cls, "__index__") or hasattr(cls, "__int__")
                              or hasattr(cls, "__float__"))
            elif mode == QUOTE_STRINGS:
                quoted = isinstance(field, str)
            elif mode == QUOTE_NOTNULL:
                quoted = field is not None

            forbid_quote = (mode == QUOTE_NONE or
                            (field is None and mode in (QUOTE_STRINGS, QUOTE_NOTNULL)))
            if not isinstance(field, str):
                field = "" if field is None else str(field)
            # Use the underlying Unicode value even when __str__ returns a subclass.
            text = "".join((field,))
            if not text and d.delimiter == " " and d.skipinitialspace:
                if forbid_quote:
                    raise Error("empty field must be quoted if delimiter is a space "
                                "and skipinitialspace is true")
                quoted = True
            if text:
                if mode == QUOTE_NONE:
                    if any(ch in text for ch in syntax):
                        if d.escapechar is None:
                            raise Error("need to escape, but no escapechar set")
                        text = "".join(d.escapechar + ch if ch in syntax else ch for ch in text)
                else:
                    quoted = quoted or any(ch in text for ch in syntax)
                    if d.escapechar is not None:
                        text = text.replace(d.escapechar, d.escapechar * 2)
                    if d.quotechar in text:
                        if d.doublequote:
                            quoted = True
                            text = text.replace(d.quotechar, d.quotechar * 2)
                        elif d.escapechar is not None:
                            text = text.replace(d.quotechar, d.escapechar + d.quotechar)
                        else:
                            raise Error("need to escape, but no escapechar set")
            if quoted:
                text = d.quotechar + text + d.quotechar
            record.append(text)
        if len(record) == 1 and not record[0]:
            if forbid_quote:
                raise Error("single empty field record must be quoted")
            record[0] = d.quotechar * 2
        return self._write(d.delimiter.join(record) + terminator)

    def writerows(self, rows, /):
        for row in rows:
            self.writerow(row)


def writer(output, /, dialect=None, **fmtparams):
    write = getattr(output, "write", None)
    if not callable(write):
        raise TypeError('argument 1 must have a "write" method')
    return _Writer(write, _dialect(dialect, **fmtparams))

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

# CPython 3.14.5 Lib/csv.py (PSF-2.0; see licenses/CPython.txt).
import types


class DictReader:
    def __init__(self, f, fieldnames=None, restkey=None, restval=None,
                 dialect="excel", *args, **kwds):
        if fieldnames is not None and iter(fieldnames) is fieldnames:
            fieldnames = list(fieldnames)
        self._fieldnames = fieldnames   # list of keys for the dict
        self.restkey = restkey          # key to catch long rows
        self.restval = restval          # default value for short rows
        self.reader = reader(f, dialect, *args, **kwds)
        self.dialect = dialect
        self.line_num = 0

    def __iter__(self):
        return self

    @property
    def fieldnames(self):
        if self._fieldnames is None:
            try:
                self._fieldnames = next(self.reader)
            except StopIteration:
                pass
        self.line_num = self.reader.line_num
        return self._fieldnames

    @fieldnames.setter
    def fieldnames(self, value):
        self._fieldnames = value

    def __next__(self):
        if self.line_num == 0:
            # Used only for its side effect.
            self.fieldnames
        row = next(self.reader)
        self.line_num = self.reader.line_num

        # unlike the basic reader, we prefer not to return blanks,
        # because we will typically wind up with a dict full of None
        # values
        while row == []:
            row = next(self.reader)
        d = dict(zip(self.fieldnames, row))
        lf = len(self.fieldnames)
        lr = len(row)
        if lf < lr:
            d[self.restkey] = row[lf:]
        elif lf > lr:
            for key in self.fieldnames[lr:]:
                d[key] = self.restval
        return d

    __class_getitem__ = classmethod(types.GenericAlias)


class DictWriter:
    def __init__(self, f, fieldnames, restval="", extrasaction="raise",
                 dialect="excel", *args, **kwds):
        if fieldnames is not None and iter(fieldnames) is fieldnames:
            fieldnames = list(fieldnames)
        self.fieldnames = fieldnames    # list of keys for the dict
        self.restval = restval          # for writing short dicts
        extrasaction = extrasaction.lower()
        if extrasaction not in ("raise", "ignore"):
            raise ValueError("extrasaction (%s) must be 'raise' or 'ignore'"
                             % extrasaction)
        self.extrasaction = extrasaction
        self.writer = writer(f, dialect, *args, **kwds)

    def writeheader(self):
        header = dict(zip(self.fieldnames, self.fieldnames))
        return self.writerow(header)

    def _dict_to_list(self, rowdict):
        if self.extrasaction == "raise":
            wrong_fields = rowdict.keys() - self.fieldnames
            if wrong_fields:
                raise ValueError("dict contains fields not in fieldnames: "
                                 + ", ".join([repr(x) for x in wrong_fields]))
        return (rowdict.get(key, self.restval) for key in self.fieldnames)

    def writerow(self, rowdict):
        return self.writer.writerow(self._dict_to_list(rowdict))

    def writerows(self, rowdicts):
        return self.writer.writerows(map(self._dict_to_list, rowdicts))

    __class_getitem__ = classmethod(types.GenericAlias)

# Adapted from CPython 3.14.5 Lib/csv.py (PSF-2.0; see licenses/CPython.txt).
from io import StringIO


def _csv_separator(char):
    # Python's Unicode \w is the alphanumeric characters and underscore.
    return char not in ('_', '"', "'", "\n") and not char.isalnum()


def _sniff_matches(chars, left_delim, right_delim):
    """Find non-overlapping matches with the leftmost start and shortest quoted field."""
    both = left_delim and right_delim
    closing = {}
    candidates = []
    n = len(chars)
    # At each opening quote, the index contains only strictly later closing quotes.
    for i in range(n - 1, -1, -1):
        quote = chars[i]
        if quote not in ('"', "'"):
            continue

        if left_delim:
            # Both a bare space delimiter and the optional space after a delimiter
            # can precede this quote. Reverse order gives the earlier start priority.
            for width in (1, 2):
                start = i - width
                if start < 0 or (width == 2 and chars[i - 1] != " "):
                    continue
                delim = chars[start]
                if _csv_separator(delim):
                    end = closing.get((quote, delim) if both else quote)
                    if end is not None:
                        candidates.append((start, end, quote, delim, width == 2))
        elif i == 0 or chars[i - 1] == "\n":
            end = closing.get(quote)
            if end is not None:
                if right_delim:
                    stop, delim, space = end
                else:
                    stop, delim, space = end, "", False
                candidates.append((i, stop, quote, delim, space))

        after = i + 1
        if right_delim:
            if after < n and _csv_separator(chars[after]):
                delim = chars[after]
                if both:
                    closing[quote, delim] = after + 1
                else:
                    space = after + 1 < n and chars[after + 1] == " "
                    closing[quote] = (after + 1 + space, delim, space)
        elif after == n or chars[after] == "\n":
            closing[quote] = after

    stop = 0
    for start, end, quote, delim, space in reversed(candidates):
        if start >= stop:
            yield quote, delim, space
            stop = end


class Sniffer:
    '''
    "Sniffs" the format of a CSV file (i.e. delimiter, quotechar)
    Returns a Dialect object.
    '''
    def __init__(self):
        # in case there is more than one possible delimiter
        self.preferred = [',', '\t', ';', ' ', ':']


    def sniff(self, sample, delimiters=None):
        """
        Returns a dialect (or None) corresponding to the sample
        """

        quotechar, doublequote, delimiter, skipinitialspace = \
                   self._guess_quote_and_delimiter(sample, delimiters)
        if not delimiter:
            delimiter, skipinitialspace = self._guess_delimiter(sample,
                                                                delimiters)

        if not delimiter:
            raise Error("Could not determine delimiter")

        class dialect(Dialect):
            _name = "sniffed"
            lineterminator = '\r\n'
            quoting = QUOTE_MINIMAL
            # escapechar = ''

        dialect.doublequote = doublequote
        dialect.delimiter = delimiter
        # _csv.reader won't accept a quotechar of ''
        dialect.quotechar = quotechar or '"'
        dialect.skipinitialspace = skipinitialspace

        return dialect


    def _guess_quote_and_delimiter(self, data, delimiters):
        # Match CPython's four quote contexts, in priority order. A delimiter
        # before and after the field must be the same character.
        if '"' not in data and "'" not in data:
            return '', False, None, 0
        chars = list(data)
        for left, right in ((True, True), (False, True), (True, False), (False, False)):
            quotes = {}
            delims = {}
            spaces = 0
            for quote, delim, space in _sniff_matches(chars, left, right):
                quotes[quote] = quotes.get(quote, 0) + 1
                if delim and (delimiters is None or delim in delimiters):
                    delims[delim] = delims.get(delim, 0) + 1
                spaces += space
            if quotes:
                break
        else:
            return '', False, None, 0

        quote = max(quotes, key=quotes.get)
        delim = max(delims, key=delims.get) if delims else ''
        skip_space = delims[delim] == spaces if delim else 0
        # This last expression has no backreferences. Only one character is
        # interpolated and is escaped for both the bracket and literal contexts.
        import re
        d = "\\" + delim if delim and delim in "\\.^$*+?{}[]()|-" else delim
        inside = "[^" + d + "\\n]*"
        pattern = ("(" + d + "|^)\\W*" + quote + inside + quote + inside + quote
                   + "\\W*(" + d + "|$)")
        return quote, re.compile(pattern, re.MULTILINE).search(data) is not None, delim, skip_space

    def _guess_delimiter(self, data, delimiters):
        """
        The delimiter /should/ occur the same number of times on
        each row. However, due to malformed data, it may not. We don't want
        an all or nothing approach, so we allow for small variations in this
        number.
          1) build a table of the frequency of each character on every line.
          2) build a table of frequencies of this frequency (meta-frequency?),
             e.g.  'x occurred 5 times in 10 rows, 6 times in 1000 rows,
             7 times in 2 rows'
          3) use the mode of the meta-frequency to determine the /expected/
             frequency for that character
          4) find out how often the character actually meets that goal
          5) the character that best meets its goal is the delimiter
        For performance reasons, the data is evaluated in chunks, so it can
        try and evaluate the smallest portion of the data possible, evaluating
        additional chunks as necessary.
        """

        data = list(filter(None, data.split('\n')))

        ascii = [chr(c) for c in range(127)] # 7-bit ASCII

        # build frequency tables
        chunkLength = min(10, len(data))
        iteration = 0
        charFrequency = {}
        modes = {}
        delims = {}
        start, end = 0, chunkLength
        while start < len(data):
            iteration += 1
            for line in data[start:end]:
                counts = {}
                for ch in line:
                    if ch < "\x7f":
                        counts[ch] = counts.get(ch, 0) + 1
                for char in ascii:
                    metaFrequency = charFrequency.get(char, {})
                    # must count even if frequency is 0
                    freq = counts.get(char, 0)
                    # value is the mode
                    metaFrequency[freq] = metaFrequency.get(freq, 0) + 1
                    charFrequency[char] = metaFrequency

            for char in charFrequency.keys():
                items = list(charFrequency[char].items())
                if len(items) == 1 and items[0][0] == 0:
                    continue
                # get the mode of the frequencies
                if len(items) > 1:
                    modes[char] = max(items, key=lambda x: x[1])
                    # adjust the mode - subtract the sum of all
                    # other frequencies
                    items.remove(modes[char])
                    modes[char] = (modes[char][0], modes[char][1]
                                   - sum(item[1] for item in items))
                else:
                    modes[char] = items[0]

            # build a list of possible delimiters
            modeList = modes.items()
            total = float(min(chunkLength * iteration, len(data)))
            # (rows of consistent data) / (number of rows) = 100%
            consistency = 1.0
            # minimum consistency threshold
            threshold = 0.9
            while len(delims) == 0 and consistency >= threshold:
                for k, v in modeList:
                    if v[0] > 0 and v[1] > 0:
                        if ((v[1]/total) >= consistency and
                            (delimiters is None or k in delimiters)):
                            delims[k] = v
                consistency -= 0.01

            if len(delims) == 1:
                delim = list(delims.keys())[0]
                skipinitialspace = (len(data[0].split(delim)) ==
                                    len(data[0].split(delim + " ")))
                return (delim, skipinitialspace)

            # analyze another chunkLength lines
            start = end
            end += chunkLength

        if not delims:
            return ('', 0)

        # if there's more than one, fall back to a 'preferred' list
        if len(delims) > 1:
            for d in self.preferred:
                if d in delims.keys():
                    skipinitialspace = (len(data[0].split(d)) ==
                                        len(data[0].split(d + " ")))
                    return (d, skipinitialspace)

        # nothing else indicates a preference, pick the character that
        # dominates(?)
        items = [(v,k) for (k,v) in delims.items()]
        items.sort()
        delim = items[-1][1]

        skipinitialspace = (len(data[0].split(delim)) ==
                            len(data[0].split(delim + " ")))
        return (delim, skipinitialspace)


    def has_header(self, sample):
        # Creates a dictionary of types of data in each column. If any
        # column is of a single type (say, integers), *except* for the first
        # row, then the first row is presumed to be labels. If the type
        # can't be determined, it is assumed to be a string in which case
        # the length of the string is the determining factor: if all of the
        # rows except for the first are the same length, it's a header.
        # Finally, a 'vote' is taken at the end for each column, adding or
        # subtracting from the likelihood of the first row being a header.

        rdr = reader(StringIO(sample), self.sniff(sample))

        header = next(rdr) # assume first row is header

        columns = len(header)
        columnTypes = {}
        for i in range(columns): columnTypes[i] = None

        checked = 0
        for row in rdr:
            # arbitrary number of rows to check, to keep it sane
            if checked > 20:
                break
            checked += 1

            if len(row) != columns:
                continue # skip rows that have irregular number of columns

            for col in list(columnTypes.keys()):
                thisType = complex
                try:
                    thisType(row[col])
                except (ValueError, OverflowError):
                    # fallback to length of string
                    thisType = len(row[col])

                if thisType != columnTypes[col]:
                    if columnTypes[col] is None: # add new column type
                        columnTypes[col] = thisType
                    else:
                        # type is inconsistent, remove column from
                        # consideration
                        del columnTypes[col]

        # finally, compare results against first row and "vote"
        # on whether it's a header
        hasHeader = 0
        for col, colType in columnTypes.items():
            if isinstance(colType, int): # it's a length
                if len(header[col]) != colType:
                    hasHeader += 1
                else:
                    hasHeader -= 1
            else: # attempt typecast
                try:
                    colType(header[col])
                except (ValueError, TypeError):
                    hasHeader += 1
                else:
                    hasHeader -= 1

        return hasHeader > 0
