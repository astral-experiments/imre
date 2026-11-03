import zlib
from zlib import adler32, compress, crc32, decompress, error as ZlibError


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
    assert ZlibError is zlib.error
    assert ZlibError.__module__ == "zlib"
    assert ZlibError.__name__ == ZlibError.__qualname__ == "error"
    assert ZlibError.__base__ is Exception and ZlibError.__bases__ == (Exception,)
    assert issubclass(ZlibError, Exception) and not issubclass(ZlibError, OSError)
    failure = ZlibError("invalid stream")
    assert isinstance(failure, zlib.error) and isinstance(failure, Exception)
    assert not isinstance(ValueError("unrelated"), zlib.error)
    try:
        raise failure
    except OSError:
        raise AssertionError("zlib.error matched OSError")
    except zlib.error as caught:
        assert caught is failure and type(caught) is ZlibError
        assert str(caught) == "invalid stream" and caught.args == ("invalid stream",)

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

    data = b"payload" * 20000
    for encode, decode in [(8, 0), (15, 47), (-9, -15), (31, 47)]:
        packed = compress(memoryview(data), level=9, wbits=encode)
        assert decompress(packed + b"trailing", wbits=decode, bufsize=1) == data
    # Interoperate with zlib and gzip streams emitted by reference zlib.
    zstream = bytes.fromhex("789c2b48acccc94f4c01000bdd02eb")
    gzip = bytes.fromhex("1f8b08000000000000032b48acccc94f4c0100156a2c4207000000")
    assert decompress(zstream, 0, 0) == b"payload"
    assert decompress(gzip + gzip, 47) == b"payload"
    assert decompress(compress(b"", 0)) == b""

    # ZIP members use raw DEFLATE and may stop mid-stream at a read limit.
    reader = zlib.decompressobj(-15)
    packed = compress(data, wbits=-15) + b"next member"
    assert reader.decompress(packed, max_length=7) == data[:7]
    assert reader.unconsumed_tail and not reader.eof
    snapshot = reader.copy()
    assert snapshot.flush(1) == data[7:]
    assert snapshot.eof and snapshot.unused_data == b"next member"
    assert not reader.eof
    assert reader.decompress(reader.unconsumed_tail) == data[7:]
    assert reader.eof and reader.unused_data == b"next member"

    # A partial stream can resume after flush. Trailing bytes accumulate at EOF.
    reader = zlib.decompressobj()
    assert reader.decompress(zstream[:-2]) == b"payload"
    assert reader.flush() == b"" and not reader.eof
    assert reader.decompress(zstream[-2:] + b"first") == b""
    assert reader.eof and reader.decompress(b"second") == b""
    assert reader.unused_data == b"firstsecond"

    data = bytearray(b"input")
    try:
        compress(data, Seed(data, True))
    except BufferError:
        pass
    else:
        raise AssertionError("resized a bytearray while exported")
    assert decompress(compress(data, Seed(data, False))) == bytes(data) == b"\xffnput"
    data.extend(b"!")

    for operation, args, error, message in [
        (compress, (b"", 10), ZlibError, "Bad compression level"),
        (decompress, (b"", 7), ZlibError, "Error -2 while preparing to decompress data: inconsistent stream state"),
        (decompress, (b"invalid",), ZlibError, "Error -3 while decompressing data: incorrect header check"),
        (decompress, (zstream[:-1],), ZlibError, "Error -5 while decompressing data: incomplete or truncated stream"),
        (decompress, (zstream, 15, -1), ValueError, "bufsize must be non-negative"),
    ]:
        try:
            operation(*args)
        except error as caught:
            assert str(caught) == message
        else:
            raise AssertionError("invalid arguments or stream accepted")

    try:
        compress(b"data", **{"other": 1, "": 2})
    except TypeError as caught:
        assert str(caught) == "compress() got an unexpected keyword argument 'other'"
    else:
        raise AssertionError("unexpected keywords accepted")
