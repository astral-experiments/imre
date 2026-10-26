import decimal
import uuid
from uuid import UUID


def show(value: UUID) -> None:
    print(isinstance(value, (decimal.Decimal, uuid.UUID)))
    print(str(value))
    print(repr(value))
    print(value.hex)
    integer = value.int
    if isinstance(integer, bool):
        print(integer is True)
    else:
        print(integer)


for text in (
    "12345678123456789abcdef001234567",
    "12345678-1234-5678-9ABC-DEF001234567",
    "{12345678-1234-5678-9abc-def001234567}",
    "urn:uuid:12345678-1234-5678-9abc-def001234567",
    "０" * 31 + "１",
    "0x_" + "f" * 29,
    "not a UUID",
    "g" * 32,
):
    try:
        show(UUID(hex=text))
    except ValueError as error:
        print(str(error))

for number in (0, 1, 1 << 127, (1 << 128) - 1, -1, 1 << 128):
    try:
        show(uuid.UUID(int=number))
    except ValueError as error:
        print(str(error))

print(isinstance("12345678123456789abcdef001234567", (decimal.Decimal, UUID)))
print(isinstance(1, UUID))
show(UUID(int=True))
show(UUID(int=False))
try:
    UUID()
except TypeError as error:
    print(str(error))
try:
    UUID("0" * 32, int=0)
except TypeError as error:
    print(str(error))
