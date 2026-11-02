import zlib
from zlib import adler32


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

    # Both module-bound functions compute the same CRC but retain independent identities.
    import binascii
    from zlib import crc32
    assert crc32 is zlib.crc32 and crc32 is not binascii.crc32
    for module in (zlib, binascii):
        crc = module.crc32
        assert crc.__self__ is module and crc.__module__ == module.__name__
        assert crc(b"123456789") == 0xCBF43926
        assert crc(b"", -(1 << 100) + 1) == 1
        assert crc(b"def", crc(b"abc")) == crc(b"abcdef")
        data = bytearray(b"x")
        try:
            crc(data, Seed(data, True))
        except BufferError:
            pass
        else:
            raise AssertionError("bytearray was resized during CRC seed conversion")
        assert crc(data, Seed(data, False)) == crc(b"\xff", -1)
        data.extend(b"!")
