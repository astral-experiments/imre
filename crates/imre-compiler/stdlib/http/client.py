"""HTTP message headers, preserving field order, spelling and duplicates."""
from __future__ import annotations

from email.message import Message, _valid_field_name

_MAXLINE = 65536
_MAXHEADERS = 100


class HTTPException(Exception):
    # Subclasses that define an __init__ must call Exception.__init__
    # or define self.args.  Otherwise, str() will fail.
    pass


class HTTPMessage(Message):
    pass


class LineTooLong(HTTPException):
    def __init__(self, line_type):
        super().__init__("got more than %d bytes when reading %s" % (_MAXLINE, line_type))


def parse_headers(fp, _class=HTTPMessage):
    lines: list[str] = []
    while True:
        raw = fp.readline(_MAXLINE + 1)
        if len(raw) > _MAXLINE:
            raise LineTooLong("header line")
        if len(lines) >= _MAXHEADERS:
            raise HTTPException("got more than %d headers" % _MAXHEADERS)
        if raw == b"\r\n" or raw == b"\n" or not raw:
            break
        lines.append(str(raw, "iso-8859-1"))
    message = _class()
    name = ""
    value = ""
    for line in lines:
        if line.startswith(" ") or line.startswith("\t"):
            if name:
                value += line
            continue
        if name:
            message.set_raw(name, value.lstrip(" \t\r\n").rstrip("\r\n"))
            name = ""
        # The MIME parser separates Unix envelope lines from header fields.
        if line.startswith("From "):
            continue
        colon = line.find(":")
        if colon < 0:
            break
        if colon == 0:
            continue
        name = line[:colon]
        if not _valid_field_name(name):
            name = ""
            break
        value = line[colon + 1:]
    if name:
        message.set_raw(name, value.lstrip(" \t\r\n").rstrip("\r\n"))
    return message
