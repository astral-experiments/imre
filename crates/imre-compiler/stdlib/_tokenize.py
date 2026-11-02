"""Read Python tokens from a text or encoded bytes readline callback."""

from _imre_tokenize import scan as _scan


_text_input = object()


class TokenizerIter:
    def __init__(self, readline, /, *, extra_tokens, encoding=_text_input):
        self._extra = bool(extra_tokens)
        if encoding is not _text_input:
            if not isinstance(encoding, str):
                name = "None" if encoding is None else type(encoding).__name__
                raise TypeError("tokenizeriter() argument 'encoding' must be str, not " + name)
            if "\x00" in encoding:
                raise ValueError("embedded null character")
        self._encoding = encoding
        self._readline = readline
        self._lineno = 0
        self._base = 0
        self._last_line = ""
        self._source = ""
        self._indents = []
        self._skipped = 0
        self._records = ()
        self._index = 0
        self._error = None
        self._done = False

    def __iter__(self):
        return self

    def _read_line(self):
        try:
            line = self._readline()
        except StopIteration:
            return ""
        if self._encoding is _text_input:
            if not isinstance(line, str):
                raise TypeError("readline() returned a non-string object")
            return line
        if not isinstance(line, bytes):
            raise TypeError("readline() returned a non-bytes object")
        return bytes.decode(line, self._encoding, "replace")

    def __next__(self):
        while self._index == len(self._records):
            if self._error is not None:
                raise self._error
            if self._done:
                raise StopIteration("EOF")

            try:
                line = self._read_line()
            except BaseException:
                # CPython invalidates the tokenizer buffer if its callback fails.
                self._error = MemoryError
                raise
            self._done = not line
            trailing_only = self._done and not self._source
            if trailing_only:
                # Exact-token mode keeps the last physical line on EOF records.
                source = self._last_line
                base = max(0, self._lineno - 1)
            else:
                if line:
                    if self._source and not self._source.endswith("\n"):
                        self._source += "\n"
                    self._source += line
                    self._last_line = line
                    self._lineno += 1
                source = self._source
                base = self._base

            self._records, indents, more, self._error = _scan(
                source, base, self._indents, self._extra, self._done,
                self._skipped, trailing_only,
            )
            self._index = 0
            if more:
                # Replay only the unfinished logical line after the next read.
                self._skipped += len(self._records)
            else:
                self._indents = indents
                self._source = ""
                self._base = self._lineno
                self._skipped = 0

        record = self._records[self._index]
        self._index += 1
        return record
