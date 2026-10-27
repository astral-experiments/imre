"""UUID values constructed from hexadecimal text or a 128-bit integer."""


def _from_hex(text: str) -> int:
    digits = text.replace("urn:", "").replace("uuid:", "").strip("{}").replace("-", "")
    if len(digits) != 32:
        raise ValueError("badly formed hexadecimal UUID string")
    return int(digits, 16)


def _number(value: int | bool) -> int:
    if isinstance(value, bool):
        return int(value)
    return value


def _value(hex: str | None, integer: int | bool | None) -> int | bool:
    if (hex is None) == (integer is None):
        raise TypeError("one of the hex, bytes, bytes_le, fields, or int arguments must be given")
    if hex is not None:
        return _from_hex(hex)
    assert integer is not None
    if isinstance(integer, bool):
        return integer
    if not 0 <= integer < (1 << 128):
        raise ValueError("int is out of range (need a 128-bit value)")
    return integer


class _UUIDValue:
    _value: int | bool

    def __init__(self, hex: str | None = None, *, int: int | bool | None = None) -> None:
        self._value = _value(hex, int)


class UUID(_UUIDValue):
    @property
    def int(self):
        return self._value

    @property
    def hex(self) -> str:
        # A leading 1 keeps all 32 digits, including the UUID's leading zeroes.
        return f"{(1 << 128) | _number(self._value):x}"[1:]

    def __str__(self) -> str:
        text = self.hex
        return "-".join((text[:8], text[8:12], text[12:16], text[16:20], text[20:]))

    def __repr__(self) -> str:
        return "UUID('" + str(self) + "')"
