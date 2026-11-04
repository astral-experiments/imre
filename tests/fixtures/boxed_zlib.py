import copy
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
    for wbits in (-15, 15):
        packed = compress(data, wbits=wbits)
        stream = zlib.decompressobj(wbits)
        assert stream.decompress(packed[:1]) == b""
        assert stream.decompress(packed[1:] + b"tail", max_length=17) == data[:17]
        assert not stream.eof and stream.unconsumed_tail
        for fork in (stream.copy(), copy.copy(stream)):
            assert fork.unconsumed_tail is stream.unconsumed_tail
            assert fork.decompress(fork.unconsumed_tail + b"!") == data[17:]
            assert fork.eof and fork.unused_data == b"tail!"
            assert not stream.eof
        pending = stream.unconsumed_tail
        assert pending is stream.unconsumed_tail
        assert stream.decompress(pending) == data[17:]
        assert stream.eof and stream.unused_data == b"tail"
        assert stream.copy().unused_data is stream.unused_data
        snapshot = stream.unused_data
        assert snapshot is stream.unused_data
        assert stream.decompress(b"!") == b""
        assert snapshot == b"tail"
        assert stream.unused_data == b"tail!"
        assert stream.flush() == b""

    # Interoperate with zlib and gzip streams emitted by reference zlib.
    zstream = bytes.fromhex("789c2b48acccc94f4c01000bdd02eb")
    gzip = bytes.fromhex("1f8b08000000000000032b48acccc94f4c0100156a2c4207000000")
    assert decompress(zstream, 0, 0) == b"payload"
    assert decompress(gzip + gzip, 47) == b"payload"
    assert decompress(compress(b"", 0)) == b""

    data = bytearray(b"input")
    try:
        compress(data, Seed(data, True))
    except BufferError:
        pass
    else:
        raise AssertionError("resized a bytearray while exported")
    assert decompress(compress(data, Seed(data, False))) == bytes(data) == b"\xffnput"
    data.extend(b"!")

    broken = zlib.decompressobj()
    for chunk in (b"bad!", zstream):
        try:
            broken.decompress(chunk)
        except ZlibError as error:
            assert str(error) == "Error -3 while decompressing data: incorrect header check"
        else:
            raise AssertionError("corrupt stream accepted")
    assert not broken.eof and broken.unconsumed_tail == zstream

    clean = zlib.compressobj()
    clean.flush()
    finished = zlib.compressobj()
    finished.flush(zlib.Z_SYNC_FLUSH)
    finished.compress(b"")
    finished.compress(b"suffix")
    finished.flush()
    for operation, args, error, message in [
        (clean.flush, (), ZlibError, "Error -2 while flushing: inconsistent stream state"),
        (finished.flush, (), ZlibError, "Error -2 while flushing: buffer error"),
        (finished.compress, (b"x",), ZlibError, "Error -2 while compressing data: buffer error"),
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
