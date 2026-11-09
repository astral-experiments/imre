from uuid import UUID

raw = b"\xff123456789abcdef"
guid = b"321\xff547689abcdef"
print(UUID(bytes=raw).bytes_le == guid)
print(UUID(None, None, guid).bytes == raw)
print(UUID(int=UUID(None, raw).int).bytes_le == guid)

try:
    UUID(bytes=b"short")
except ValueError as exc:
    print(str(exc))
try:
    UUID(bytes_le=b"")
except ValueError as exc:
    print(str(exc))
try:
    UUID(bytes=raw, int=0)
except TypeError as exc:
    print(str(exc))
