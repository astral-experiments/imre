# ruff: noqa: B033
# Duplicate literal keys exercise exact byte hashing and equality.


class Packet:
    payload: bytes
    fallback: bytes | None

    def __init__(self, payload: bytes, fallback: bytes | None = None) -> None:
        self.payload = payload
        self.fallback = fallback

    def append(self, data: bytes) -> bytes:
        self.payload = self.payload + data
        return self.payload


def pair(value: bytes) -> tuple[bytes, bytes | None, int]:
    return value, value[::-1], len(value)


def maybe(value: bytes | None) -> bytes:
    return value if value is not None else b"missing"


packet = Packet(bytes([0, 128, 255]))
alias = packet
values: list[bytes] = [packet.payload, b"tail"]
copy = values[:]
repeated = values * 2
packet.payload = b"new"
values[0] = b"replacement"
print(alias.append(b"!"))
print(copy[0])
print(repeated[0])
print(repeated[2])
values.append(copy.pop())
print(values.pop())
first, second, count = pair(packet.payload)
print(first)
print(maybe(second))
print(count)
print(maybe(packet.fallback))
packet.fallback = first
print(maybe(packet.fallback))

nullable: list[bytes | None] = [b"owned", None, bytes([255])]
for value in nullable:
    print(maybe(value))

mapping: dict[bytes, bytes | None] = {b"key": first, b"\0\xff": None}
same_key = bytes([107, 101, 121])
print(same_key in mapping)
print(maybe(mapping[same_key]))
print(maybe(mapping.get(b"absent")))
print(maybe(mapping.get(b"absent", b"default")))
mapping[same_key] = b"updated"
for key, value in mapping.items():
    print(key)
    print(maybe(value))
del mapping[b"\0\xff"]
print(len(mapping))
try:
    print(maybe(mapping[b"missing\0\xff"]))
except KeyError as error:
    print(error)

unique: set[bytes] = {b"a", b"a", b"\0\xff"}
unique.add(bytes([97]))
print(len(unique))
print(b"a" in unique)
unique.discard(b"a")
unique.discard(b"absent")
for member in unique:
    print(member)
keys: set[bytes] = {bytes([value]) for value in b"abca"}
print(len(keys))
print(b"b" in keys)
table: dict[bytes, bytes] = {bytes([value]): bytes([value])[::-1] for value in b"abca"}
for key, entry in table.items():
    print(key)
    print(entry)
