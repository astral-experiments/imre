"""Typed facade for the rebuilt google-crc32c extension."""

from typing import Self

from ._crc32c import extend, value


class Checksum:
    _crc: int

    def __init__(self, initial_value: bytes = b"") -> None:
        self._crc = value(initial_value)

    def update(self, chunk: bytes) -> None:
        self._crc = extend(self._crc, chunk)

    def digest(self) -> bytes:
        crc = self._crc
        return bytes([crc >> 24, (crc >> 16) & 255, (crc >> 8) & 255, crc & 255])

    def hexdigest(self) -> bytes:
        digits = b"0123456789abcdef"
        result: list[int] = []
        for byte in self.digest():
            result.append(digits[byte >> 4])
            result.append(digits[byte & 15])
        return bytes(result)

    def copy(self) -> Self:
        clone = Checksum()
        clone._crc = self._crc
        return clone
