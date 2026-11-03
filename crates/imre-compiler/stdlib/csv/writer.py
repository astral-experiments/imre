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
