"""Regex helpers and scanner loop from CPython 3.14.5 Lib/re (PSF-2.0)."""

class PatternError(Exception):
    """Exception raised for invalid regular expressions.

    Attributes:

        msg: The unformatted error message
        pattern: The regular expression pattern
        pos: The index in the pattern where compilation failed (may be None)
        lineno: The line corresponding to pos (may be None)
        colno: The column corresponding to pos (may be None)
    """

    __module__ = 're'

    def __init__(self, msg, pattern=None, pos=None):
        self.msg = msg
        self.pattern = pattern
        self.pos = pos
        if pattern is not None and pos is not None:
            msg = '%s at position %d' % (msg, pos)
            newline = '\n' if isinstance(pattern, str) else 10
            self.lineno = 1
            self.colno = pos + 1
            for index, char in enumerate(pattern[:pos]):
                if char == newline:
                    self.lineno += 1
                    self.colno = pos - index
            if newline in pattern:
                msg = '%s (line %d, column %d)' % (msg, self.lineno, self.colno)
        else:
            self.lineno = self.colno = None
        super().__init__(msg)


# Backward compatibility after renaming in 3.13
error = PatternError


# SPECIAL_CHARS
# closing ')', '}' and ']'
# '-' (a range in character set)
# '&', '~', (extended character set operations)
# '#' (comment) and WHITESPACE (ignored) in verbose mode
_special_chars_map = {i: '\\' + chr(i) for i in b'()[]{}?*+-|^$\\.&~# \t\n\r\v\f'}

def escape(pattern):
    """
    Escape special characters in a string.
    """
    if isinstance(pattern, str):
        return pattern.translate(_special_chars_map)
    else:
        pattern = str(pattern, 'latin1')
        return pattern.translate(_special_chars_map).encode('latin1')


from re import compile as _compile, split as _split


class _ZeroSentinel(int):
    pass
_zero_sentinel = _ZeroSentinel()

def sub(pattern, repl, string, *args, count=_zero_sentinel, flags=_zero_sentinel):
    """Return the string obtained by replacing the leftmost
    non-overlapping occurrences of the pattern in string by the
    replacement repl.  repl can be either a string or a callable;
    if a string, backslash escapes in it are processed.  If it is
    a callable, it's passed the Match object and must return
    a replacement string to be used."""
    if args:
        if count is not _zero_sentinel:
            raise TypeError("sub() got multiple values for argument 'count'")
        count, *args = args
        if args:
            if flags is not _zero_sentinel:
                raise TypeError("sub() got multiple values for argument 'flags'")
            flags, *args = args
            if args:
                raise TypeError("sub() takes from 3 to 5 positional arguments "
                                "but %d were given" % (5 + len(args)))

        import warnings
        warnings.warn(
            "'count' is passed as positional argument",
            DeprecationWarning, stacklevel=2
        )

    return _compile(pattern, flags).sub(repl, string, count)
sub.__text_signature__ = '(pattern, repl, string, count=0, flags=0)'

def split(pattern, string, *args, maxsplit=_zero_sentinel, flags=_zero_sentinel):
    """Split the source string by the occurrences of the pattern,
    returning a list containing the resulting substrings.  If
    capturing parentheses are used in pattern, then the text of all
    groups in the pattern are also returned as part of the resulting
    list.  If maxsplit is nonzero, at most maxsplit splits occur,
    and the remainder of the string is returned as the final element
    of the list."""
    if args:
        if maxsplit is not _zero_sentinel:
            raise TypeError("split() got multiple values for argument 'maxsplit'")
        maxsplit, *args = args
        if args:
            if flags is not _zero_sentinel:
                raise TypeError("split() got multiple values for argument 'flags'")
            flags, *args = args
            if args:
                raise TypeError("split() takes from 2 to 4 positional arguments "
                                "but %d were given" % (4 + len(args)))

        import warnings
        warnings.warn(
            "'maxsplit' is passed as positional argument",
            DeprecationWarning, stacklevel=2
        )

    return _split(pattern, string, maxsplit, flags)
split.__text_signature__ = '(pattern, string, maxsplit=0, flags=0)'

class Scanner:
    def __init__(self, lexicon, flags=0):
        from re import _compile_scanner
        self.lexicon = lexicon
        self.scanner = _compile_scanner(lexicon, flags)

    def scan(self, string):
        result = []
        append = result.append
        match = self.scanner.scanner(string).match
        i = 0
        while True:
            m = match()
            if not m:
                break
            j = m.end()
            if i == j:
                break
            action = self.lexicon[m.lastindex-1][1]
            if callable(action):
                self.match = m
                action = action(self, m.group())
            if action is not None:
                append(action)
            i = j
        return result, string[i:]
