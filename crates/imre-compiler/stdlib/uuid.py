"""UUID values constructed from hexadecimal text, integers, or operating-system randomness."""

RESERVED_NCS = "reserved for NCS compatibility"
RFC_4122 = "specified in RFC 4122"
RESERVED_MICROSOFT = "reserved for Microsoft compatibility"
RESERVED_FUTURE = "reserved for future definition"


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
        return self.bytes.hex()

    @property
    def bytes(self):
        return self.int.to_bytes(16)

    @property
    def variant(self) -> str:
        value = _number(self._value)
        if not value & (1 << 63):
            return RESERVED_NCS
        if not value & (1 << 62):
            return RFC_4122
        if not value & (1 << 61):
            return RESERVED_MICROSOFT
        return RESERVED_FUTURE

    @property
    def version(self):
        if self.variant == RFC_4122:
            return (_number(self._value) >> 76) & 0xf
        return None

    def __str__(self) -> str:
        text = self.hex
        return "-".join((text[:8], text[8:12], text[12:16], text[16:20], text[20:]))

    def __repr__(self) -> str:
        return "UUID('" + str(self) + "')"
