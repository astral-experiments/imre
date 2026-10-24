import hashlib
from hashlib import sha256 as checksum


def make_data() -> bytes:
    print("evaluated")
    return b"abc"


print(checksum().hexdigest())
print(hashlib.sha256(make_data()).hexdigest())
for size in [1, 55, 56, 63, 64, 65, 119, 120, 127, 128, 129]:
    print(checksum(b"\xff" * size).hexdigest())
print(checksum(bytes([0, 255, 128, 0])).hexdigest())
