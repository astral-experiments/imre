"""Regex exception and scanner loop from CPython 3.14.5 Lib/re (PSF-2.0)."""

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
