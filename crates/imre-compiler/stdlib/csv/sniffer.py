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
