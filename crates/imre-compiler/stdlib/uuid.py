"""UUID values constructed from hexadecimal text, bytes, integers, or operating-system randomness."""

RESERVED_NCS = "reserved for NCS compatibility"
RFC_4122 = "specified in RFC 4122"
RESERVED_MICROSOFT = "reserved for Microsoft compatibility"
RESERVED_FUTURE = "reserved for future definition"


def _from_hex(text: str) -> int:
    digits = text.replace("urn:", "").replace("uuid:", "").strip("{}").replace("-", "")
    if len(digits) != 32:
        raise ValueError("badly formed hexadecimal UUID string")
    return int(digits, 16)


def _swap_fields(data: bytes) -> bytes:
    # Little-endian UUIDs reverse the first three fields, leaving the last eight bytes intact.
    return data[3::-1] + data[5:3:-1] + data[7:5:-1] + data[8:]


def _value(
    hex: str | None, data: bytes | None, little: bytes | None, integer: int | bool | None
) -> int:
    if (hex is not None) + (data is not None) + (little is not None) + (integer is not None) != 1:
        raise TypeError("one of the hex, bytes, bytes_le, fields, or int arguments must be given")
    if hex is not None:
        return _from_hex(hex)
    if data is not None:
        if len(data) != 16:
            raise ValueError("bytes is not a 16-char string")
        return int(data.hex(), 16)
    if little is not None:
        if len(little) != 16:
            raise ValueError("bytes_le is not a 16-char string")
        return int(_swap_fields(little).hex(), 16)
    assert integer is not None
    if isinstance(integer, bool):
        return int(integer)
    if not 0 <= integer < (1 << 128):
        raise ValueError("int is out of range (need a 128-bit value)")
    return integer


class _UUIDValue:
    _value: int
    _was_bool: bool

    def __init__(
        self,
        hex: str | None = None,
        bytes: bytes | None = None,
        bytes_le: bytes | None = None,
        *,
        int: int | bool | None = None,
    ) -> None:
        self._value = _value(hex, bytes, bytes_le, int)
        self._was_bool = isinstance(int, bool)


class UUID(_UUIDValue):
    @property
    def int(self):
        if self._was_bool:
            return bool(self._value)
        return self._value

    @property
    def hex(self) -> str:
        return self.bytes.hex()

    @property
    def bytes(self):
        return self._value.to_bytes(16)

    @property
    def bytes_le(self):
        return _swap_fields(self.bytes)

    @property
    def variant(self) -> str:
        value = self._value
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
            return (self._value >> 76) & 0xf
        return None

    def __str__(self) -> str:
        text = self.hex
        return "-".join((text[:8], text[8:12], text[12:16], text[16:20], text[20:]))

    def __repr__(self) -> str:
        return "UUID('" + str(self) + "')"
