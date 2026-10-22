"""Native default-base integer boundary used by the generated applications."""

import unicodedata


def _checked_int(value: str) -> int:
    """Reject ASCII control separators before native integer conversion."""
    if value and ord(value[0]) > 32 and ord(value[-1]) > 32:
        return int(value)
    trimmed = value.strip(" \t\n\r\v\f")
    if not trimmed or "\x1c" <= trimmed[0] <= "\x1f" or "\x1c" <= trimmed[-1] <= "\x1f":
        raise ValueError("invalid literal for int() with base 10")
    return int(trimmed)


def parse_int(value):
    """Normalize Python's Unicode decimal input before Codon's signed-int parser."""
    if isinstance(value, str):
        if value.isascii():
            return _checked_int(value)
        parts: list[str] = []
        for character in value:
            digit = unicodedata.decimal(character, -1)
            if digit >= 0:
                parts.append("0123456789"[digit])
            elif ord(character) >= 128 and character.isspace():
                parts.append(" ")
            else:
                parts.append(character)
        return _checked_int("".join(parts))
    else:
        return int(value)
