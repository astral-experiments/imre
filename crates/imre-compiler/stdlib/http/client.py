"""HTTP message headers, preserving field order, spelling and duplicates."""
from __future__ import annotations

from email.message import Message, _valid_field_name


class HTTPException(Exception):
    # Subclasses that define an __init__ must call Exception.__init__
    # or define self.args.  Otherwise, str() will fail.
    pass


class HTTPMessage(Message):
    def _parse(self, lines: list[str]) -> None:
        name = ""
        value = ""
        for line in lines:
            if line.startswith(" ") or line.startswith("\t"):
                if name:
                    value += line
                continue
            if name:
                self.set_raw(name, value.lstrip(" \t\r\n").rstrip("\r\n"))
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
            self.set_raw(name, value.lstrip(" \t\r\n").rstrip("\r\n"))
