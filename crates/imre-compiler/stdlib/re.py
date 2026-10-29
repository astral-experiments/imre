"""Regex exception from CPython 3.14.5 Lib/re/_constants.py (PSF-2.0)."""

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
