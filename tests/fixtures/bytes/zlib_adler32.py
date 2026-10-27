from zlib import adler32


class Seed:
    def __index__(self) -> int:
        print("index")
        return -(1 << 100) + 7


def data() -> bytes:
    print("data")
    return b"abcdef"


def seed() -> Seed:
    print("seed")
    return Seed()


for value in [1, 0, 432, -1, 0xFFFF, 0xFFFF0000, 1 << 100, -(1 << 100) + 7]:
    print(adler32(b"", value))
    print(adler32(b"penguin", value))

print(adler32(b"abc"))
print(adler32(b"def", adler32(b"abc")) == adler32(b"abcdef"))
print(adler32(b"x" * 8193, True))
# zlib's one-byte fast path preserves a noncanonical high half.
print(adler32(b"\x00", 0xFFFF_FFF0))
print(adler32(data(), seed()))
try:
    print(adler32("not bytes", seed()))
except TypeError as error:
    print(str(error))
try:
    print(adler32(b"value", "not an integer"))
except TypeError as error:
    print(str(error))

# Werkzeug uses the encoded path to distinguish files with the same metadata.
path = "/café/report.txt"
check = adler32(path.encode()) & 0xFFFFFFFF
print(f"{123}-{32}-{check}")
