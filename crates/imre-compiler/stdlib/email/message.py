"""Ordered message headers for email and HTTP."""
from __future__ import annotations


def _valid_field_name(name: str) -> bool:
    for char in name:
        if char == ":" or not 33 <= ord(char) <= 126:
            return False
    return bool(name)


class Message:
    _headers: list[tuple[str, str]]

    def __init__(self) -> None:
        self._headers = []

    def items(self) -> list[tuple[str, str]]:
        return self._headers[:]

    def get(self, name: str, failobj: str | None = None) -> str | None:
        name = name.lower()
        for field, value in self._headers:
            if field.lower() == name:
                return value
        return failobj

    def __getitem__(self, name: str) -> str | None:
        return self.get(name)

    def __setitem__(self, name: str, value: str) -> None:
        if not _valid_field_name(name):
            raise ValueError("Header field name contains invalid characters: " + repr(name))
        self._headers.append((name, value))

    def get_all(self, name: str, failobj: list[str] | None = None) -> list[str] | None:
        values: list[str] = []
        name = name.lower()
        for field, value in self._headers:
            if field.lower() == name:
                values.append(value)
        if values:
            return values
        return failobj
