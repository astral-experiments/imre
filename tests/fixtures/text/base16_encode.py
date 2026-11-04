from array import array
from base64 import b16encode

data = bytearray(b"\x00\xaf\xff\x10")
for source in (b"", data, memoryview(data)[1:3], array("B", data)):
    print(b16encode(s=source))
print(data)

for invalid in ("abc", [65], memoryview(data)[::2]):
    try:
        b16encode(invalid)
    except (TypeError, BufferError) as exc:
        print(type(exc).__name__, str(exc))
