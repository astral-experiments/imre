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
    from binascii import a2b_qp

    print(a2b_qp("tag_name=5F=FF", header=True))
    print(a2b_qp((b"x" * 64 + b"=4A") * 2 + b"=QZ=\rignored\n=x=09_"))
    print(a2b_qp(b"u_x=2D" * 7 + b"a" * 130 + b"=5F_", header=True))
    print(a2b_qp(memoryview(b".=00==z=\rdiscard\n!")[1:-1]))
    raw = bytearray(b"=41")
    try:
        a2b_qp(raw, header=Resize(raw))
    except BufferError as error:
        print(str(error))
    raw.append(33)
    print(a2b_qp(raw))

    print(binascii.b2a_hex is binascii.hexlify)
    for encode in (binascii.b2a_hex, binascii.hexlify):
        print(encode.__name__, encode(memoryview(b".\xfa\x00")[1:]))
        print(encode(data=b"ABC", sep="ÿ", bytes_per_sep=-2))
    for decode in (binascii.a2b_hex, binascii.unhexlify):
        print(repr(decode), decode.__name__, decode.__qualname__, decode.__module__,
              decode.__text_signature__, decode.__self__ is binascii)
        for value in (b"AbCf", bytearray(b"FF00"), "00aa", chr(0xd800), "é?", b"$", b"0$"):
            try:
                print(decode(value))
            except (ValueError, TypeError) as error:
                print(type(error).__name__, str(error))
    from binascii import Error, Incomplete, a2b_base64, b2a_base64, crc32

    data = memoryview(bytearray(b".123456789."))[1:-1]
    print(crc32 is binascii.crc32, crc32(data), crc32(data[4:], crc32(data[:4])))
    print(crc32(b"", -(1 << 80) - 1))

    from binascii import b2a_qp

    print(b2a_qp(b"Lines: \xff \r\n.\r\n"), b2a_qp(b"first last_", header=True))
    print(b2a_qp(data=b"x" * 74 + b"\xff", istext=False))
    assert b2a_qp(b"x" * 75 + b"=\r\n") == b"x" * 75 + b"=\r\n=3D\r\n"
    assert b2a_qp(b"x" * 74 + b" \n.\n") == b"x" * 74 + b"=20\n=2E\n"
    raw = bytearray(b"A\x00")
    try:
        b2a_qp(raw, quotetabs=Resize(raw))
    except BufferError as error:
        print(str(error))
    raw.append(33)
    print(b2a_qp(raw))

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
