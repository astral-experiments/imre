"""HTTP message headers, preserving field order, spelling and duplicates."""
from __future__ import annotations


class HTTPMessage:
    _headers: list[tuple[str, str]]

    def __init__(self) -> None:
        self._headers = []

    def items(self) -> list[tuple[str, str]]:
        return self._headers[:]

    def get(self, name: str, failobj: str | None = None) -> str | None:
        for field, value in self._headers:
            if field.lower() == name.lower():
                return value
        return failobj

    def __getitem__(self, name: str) -> str | None:
        return self.get(name)

    def __setitem__(self, name: str, value: str) -> None:
        self._headers.append((name, value))

    def get_all(self, name: str) -> list[str] | None:
        values: list[str] = []
        for field, value in self._headers:
            if field.lower() == name.lower():
                values.append(value)
        if values:
            return values
        return None

    def _parse(self, lines: list[str]) -> None:
        name = ""
        value = ""
        for line in lines:
            if line.startswith(" ") or line.startswith("\t"):
                if name:
                    value += line
                continue
            if name:
                self._headers.append((name, value.lstrip(" \t\r\n").rstrip("\r\n")))
                name = ""
            colon = line.find(":")
            if colon < 0:
                break
            if colon == 0:
                continue
            name = line[:colon]
            for char in name:
                if ord(char) < 33 or ord(char) > 126:
                    name = ""
                    break
            if not name:
                break
            value = line[colon + 1:]
        if name:
            self._headers.append((name, value.lstrip(" \t\r\n").rstrip("\r\n")))
