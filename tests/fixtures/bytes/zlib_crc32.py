from binascii import crc32 as binary_crc
from zlib import crc32

assert crc32(b"123456789") == 0xCBF43926
assert binary_crc(b"123456789") == 0xCBF43926
assert crc32(b"", -(1 << 100) - 7) == 4294967289
assert binary_crc(b"penguin", (1 << 100) + 1) == 1136044692
assert crc32(b"def", crc32(b"abc")) == crc32(b"abcdef")
