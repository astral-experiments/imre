import zlib
from zlib import adler32, crc32


class Seed:
    def __init__(self, data, resize):
        self.data = data
        self.resize = resize

    def __index__(self):
        if self.resize:
            self.data.extend(b"!")
        self.data[0] = 255
        return -1


def current():
    print(adler32 is zlib.adler32, type(adler32) is type(len), adler32.__self__ is zlib)
    print(adler32.__module__, adler32.__name__, adler32.__qualname__)
    print(adler32("résumé.csv".encode()) & 0xFFFFFFFF)
    print(adler32(b"", -1), adler32(b"\0", 0xFFFFFFF0), adler32(b"xyz", (1 << 100) + 1))
    print(crc32 is zlib.crc32, crc32.__self__ is zlib, crc32.__module__, crc32.__name__)
    print(crc32(b"123456789"), crc32(b"", -1), crc32(b"abc", (1 << 100) + 7))
    print(crc32(b"789", crc32(b"123456")) == crc32(b"123456789"))
    data = bytearray(b"x")
    try:
        crc32(data, Seed(data, True))
    except BufferError as error:
        print(str(error), data)
    print(crc32(data, Seed(data, False)), data)
    data = bytearray(b"x")
    try:
        adler32(data, Seed(data, True))
    except BufferError as error:
        print(str(error), data)
    print(adler32(data, Seed(data, False)), data)
    data.extend(b"!")
    for args in ((), ("text",), (b"", "seed"), (b"", 1, 2)):
        try:
            adler32(*args)
        except TypeError as error:
            print(str(error))
