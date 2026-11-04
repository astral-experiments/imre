import binascii


class Resize:
    def __init__(self, data, delete=False):
        self.data = data
        self.delete = delete

    def __bool__(self):
        if self.delete:
            del self.data[::-2]
        else:
            self.data.extend(b"!")
        return True


class DropIndex:
    def __init__(self, data):
        self.data = data

    def __index__(self):
        del self.data[:]
        return 0


def current():
    for decode in (binascii.a2b_hex, binascii.unhexlify):
        print(repr(decode), decode.__name__, decode.__qualname__, decode.__module__,
              decode.__text_signature__, decode.__self__ is binascii)
        for value in (b"AbCf", bytearray(b"FF00"), "00aa", chr(0xd800), "é?", b"$", b"0$"):
            try:
                print(decode(value))
            except (ValueError, TypeError) as error:
                print(type(error).__name__, str(error))
    from binascii import Error, Incomplete, a2b_base64, b2a_base64, crc32

    assert crc32 is binascii.crc32 and crc32.__self__ is binascii
    assert crc32(b"789", crc32(b"123456")) == 0xCBF43926
    data = bytearray(b"abc")
    try:
        crc32(data, DropIndex(data))
    except BufferError:
        assert data == b"abc"
    else:
        raise AssertionError("resized the checksum input during seed conversion")

    print(type(a2b_base64) is type(len), a2b_base64 is binascii.a2b_base64)
    print(Error.__module__, Error.__name__, Error.__bases__ == (ValueError,))
    print(issubclass(Error, ValueError), Incomplete.__base__ is Exception)
    for data in (b"", b"\x00\xfb\xff", bytearray(b"Python")):
        encoded = b2a_base64(data, newline=False)
        print(encoded, a2b_base64(encoded, strict_mode=True) == bytes(data))
    print(b2a_base64(b"abc"), a2b_base64("Y!WJj"))
    for data in ("é", 42, b"A", b"=YWJj", b"Y!WJj", b"YQ==QQ=="):
        try:
            a2b_base64(data, strict_mode=True)
        except (ValueError, TypeError) as error:
            print(type(error).__name__, str(error), type(error) is Error)
    try:
        a2b_base64(b"A")
    except Error as error:
        print(isinstance(error, ValueError), error.args)
    data = bytearray(b"YWJj")
    for delete in (False, True):
        try:
            a2b_base64(data, strict_mode=Resize(data, delete))
        except BufferError as error:
            print(str(error), data)
    del data[::-2]
    drop = DropIndex(data)
    del data[drop:4]
    data.extend(b"!")
    print(data)
