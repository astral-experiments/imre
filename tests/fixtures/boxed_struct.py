import struct


def current():
    from struct import error

    assert error is struct.error and error.__bases__ == (Exception,)
    assert (error.__module__, error.__name__, error.__qualname__) == ("struct", "error", "error")
    saved = struct.Struct(">I")
    struct.error = ValueError
    try:
        try:
            saved.unpack(b"x")
        except error as caught:
            assert type(caught) is error
            assert caught.args == ("unpack requires a buffer of 4 bytes",)
        else:
            raise AssertionError("short struct buffer was accepted")
    finally:
        struct.error = error

    for prefix in ("", "@", "=", "<", ">", "!"):
        layout = struct.Struct(prefix + "B2xI")
        assert layout.size == struct.calcsize(prefix + "3BI")
        packed = layout.pack(17, 42)
        assert packed[1:3] == b"\0\0"
        assert layout.unpack(packed[:1] + b"ab" + packed[3:]) == (17, 42)
    assert struct.Struct("0x").pack() == b""
    assert struct.unpack("2x", b"ab") == ()
    assert struct.calcsize(b">0x3xB") == 4
    try:
        struct.Struct("2x").pack(1)
    except struct.error as error:
        print(str(error))
    else:
        raise AssertionError("padding consumed a value")

    time_format = struct.Struct(format=b">Q")
    pack = time_format.pack
    unpack = time_format.unpack
    print(time_format.format, time_format.size, isinstance(time_format, struct.Struct))
    for value in (0, 1700000000, (1 << 64) - 1):
        raw = pack(value)
        print(list(raw), unpack(bytearray(raw)))
    for value in (-1, 1 << 64):
        try:
            pack(value)
        except struct.error as error:
            print(str(error))
    for args in ((), (1, 2)):
        try:
            pack(*args)
        except struct.error as error:
            print(str(error))
    try:
        unpack(b"short")
    except struct.error as error:
        print(str(error))

    order = []
    class Field:
        def __init__(self, number):
            self.number = number
        def __index__(self):
            order.append(self.number)
            return self.number

    record = struct.Struct("<B2HI")
    print(list(record.pack(Field(1), Field(2), Field(258), Field(66051))), order)
    try:
        record.pack(Field(300), Field(4), Field(5), Field(6))
    except struct.error as error:
        print(str(error), order)
    pack = struct.Struct(">H").pack
    for _ in range(5000):
        discarded = [None]
    print(list(pack(258)))

    class Text(str):
        pass

    class Data(bytes):
        def __bytes__(self):
            raise AssertionError("buffer converted")

    class Buffer(bytearray):
        def __len__(self):
            raise AssertionError("length called")

    header = struct.Struct("!2cI")
    assert header.pack(Data(b"M"), b"\xff", 258) == b"M\xff\0\0\x01\x02"
    assert header.unpack_from(b"XM\xff\0\0\x01\x02", 1) == (b"M", b"\xff", 258)
    target = bytearray(b"1234")
    try:
        struct.pack_into("2c", target, 1, b"A", bytearray(b"B"))
    except struct.error as error:
        assert str(error) == "char format requires a bytes object of length 1"
        assert target == b"1A\x004"
    else:
        raise AssertionError("accepted a bytearray for a char field")

    pascal = struct.Struct("!0p4p")
    target = bytearray(b"______")
    pascal.pack_into(target, 1, b"ignored", Data(b"abcde"))
    assert target == b"_\x03abc_"
    assert pascal.unpack_from(target, 1) == (b"", b"abc")
    assert struct.unpack("3p", b"\xffab") == (b"ab",)
    wide = struct.pack("260p", Buffer(b"x" * 259))
    assert wide == b"\xff" + b"x" * 259
    assert struct.unpack("260p", wide) == (b"x" * 255,)
    alias = bytearray(b"abcd")
    struct.pack_into("4p", alias, 0, alias)
    assert alias == b"\x03\0\0\0"

    for fmt in (Text(">Q"), Data(b">Q")):
        record = struct.Struct(fmt)
        print(record.format, struct.calcsize(fmt))
        for raw in (bytearray(8), Data(bytes(8)), Buffer(bytes(8))):
            print(record.unpack(raw), struct.unpack(fmt, raw))
        try:
            struct.unpack(fmt, Buffer(b"short"))
        except struct.error as error:
            print(str(error))

    # End of central directory record with a fixed byte string and repeated integers.
    fmt = b"<4s4H2LH"
    values = (b"PK\x05\x06", 0, 0, 1, 1, 55, 99, 0)
    record = struct.Struct(fmt)
    data = struct.pack(fmt, *values)
    assert struct.calcsize(fmt) == record.size == len(data) == 22
    assert record.unpack(data) == struct.unpack(fmt, data) == values
    assert record.pack(*values) == data
    print(list(data))

    for prefix in ("<", ">", "=", "@", "!"):
        fmt = prefix + "B0s3s2Ls"
        values = (1, b"ignored", Buffer(b"z"), 2, 0xfedcba98, Data(b"tail"))
        encoded = struct.pack(fmt, *values)
        assert struct.unpack(fmt, encoded) == (1, b"", b"z\0\0", 2, 0xfedcba98, b"t")
        print(list(encoded))
    for fmt, value in (("0s", 0), ("s", "text"), ("<L", -1), ("@L", 1 << 70)):
        try:
            struct.pack(fmt, value)
        except struct.error as error:
            print(str(error))
        else:
            raise AssertionError("invalid struct input accepted")

    # A failed field conversion leaves the preceding fields written.
    data = bytearray(b"abcdefgh")
    class TooLarge:
        def __index__(self):
            assert data == b"a\x07\x00\x00efgh"
            return 1 << 16

    try:
        struct.pack_into(">BH", data, 1, 7, TooLarge())
    except struct.error:
        assert data == b"a\x07\x00\x00efgh"
    else:
        raise AssertionError("field overflow accepted")
    view = memoryview(data)[1:6]
    assert struct.Struct(">H").pack_into(view, -2, 0x1234) is None
    assert data == b"a\x07\x00\x00\x12\x34gh"

    # Read a signed field within a larger record from either end of a mutable buffer.
    field = struct.Struct(">l")
    data = bytearray(b"ab" + field.pack(-2) + b"c")
    assert struct.unpack_from(">l", data, 2) == field.unpack_from(buffer=data, offset=-5) == (-2,)
    try:
        field.unpack_from(data, -3)
    except struct.error as error:
        assert str(error) == "not enough data to unpack 4 bytes at offset -3"
    else:
        raise AssertionError("short field accepted")

    record = struct.Struct(">lbb")
    assert record.size == 6
    encoded = b"\xff\xff\xff\xff\x80\x7f"
    assert record.unpack(encoded) == (-1, -128, 127)
    assert record.pack(-1, Field(-128), 127) == encoded

    class HookOverflow(OverflowError):
        pass

    class OverflowingIndex:
        def __index__(self):
            raise HookOverflow("conversion failed")

    for prefix in ("", "@", "=", "<", ">", "!"):
        fmt = prefix + "l"
        width = struct.calcsize(fmt)
        assert width == (struct.calcsize("@L") if prefix in ("", "@") else 4)
        limit = 1 << (8 * width - 1)
        words = (-limit, -7, 0, limit - 1)
        layout = struct.Struct(prefix + "4l")
        packed = layout.pack(*words)
        assert layout.unpack(bytearray(packed)) == words
        assert struct.unpack(fmt, struct.pack(fmt, Field(-12))) == (-12,)
        for n in (-limit - 1, limit, 1 << 1000, OverflowingIndex()):
            try:
                struct.pack(fmt, n)
            except struct.error as error:
                assert str(error) == "'l' format requires %d <= number <= %d" % (-limit, limit - 1)
            else:
                raise AssertionError("signed long overflow accepted")
    assert struct.pack(">l", -2) == b"\xff\xff\xff\xfe"
    assert struct.unpack("<l", b"\xfe\xff\xff\xff") == (-2,)
    assert struct.unpack("!0l", b"") == ()
    transitions = (-(1 << 63), -2208988800, (1 << 63) - 1)
    for prefix in ("", "@", "=", "<", ">", "!"):
        layout = struct.Struct(prefix + "3q")
        assert layout.size == 24
        assert layout.unpack(layout.pack(*transitions)) == transitions
        for n in (-(1 << 63) - 1, 1 << 63):
            try:
                struct.pack(prefix + "q", n)
            except struct.error as error:
                assert str(error) == "'q' format requires -9223372036854775808 <= number <= 9223372036854775807"
            else:
                raise AssertionError("signed long long overflow accepted")
    assert struct.pack(">q", Field(-2)) == b"\xff" * 7 + b"\xfe"
    assert struct.unpack("<q", b"\xfe" + b"\xff" * 7) == (-2,)
    opposite_endian = ">" if struct.pack("@q", 1) == struct.pack("<q", 1) else "<"
    try:
        struct.pack(opposite_endian + "q", OverflowingIndex())
    except HookOverflow:
        pass
    else:
        raise AssertionError("lost index error")

    # The length field of a native cmsghdr has type size_t.
    header = struct.Struct("@Nii")
    assert header.unpack(header.pack(Field(20), 1, 1)) == (20, 1, 1)
    bits = 8 * struct.calcsize("P")
    assert struct.calcsize("@B0N") == struct.calcsize("@B0n") == struct.calcsize("@B0P")
    for code, low, high in (("n", -(1 << (bits - 1)), (1 << (bits - 1)) - 1),
                            ("N", 0, (1 << bits) - 1)):
        pair = struct.Struct("@2" + code)
        assert pair.unpack(pair.pack(low, high)) == (low, high)
        for value in (low - 1, high + 1, OverflowingIndex()):
            try:
                struct.pack(code, value)
            except struct.error as error:
                assert str(error) == f"'{code}' format requires {low} <= number <= {high}"
            else:
                raise AssertionError("native size overflow accepted")

    sample = struct.Struct("<h?")
    assert sample.pack(Field(-32768), [1]) == b"\x00\x80\x01"
    value, flag = sample.unpack(b"\xff\x7f\x80")
    assert value == 32767 and flag is True
    assert struct.pack(">h?", -2, []) == b"\xff\xfe\x00"

    assert struct.calcsize("@B0l") == struct.calcsize("@B0L")
    bcm = struct.Struct("@3I4l2I")
    fields = (1, 2, 3, -1, 17, -19, 23, 29, 31)
    assert bcm.unpack(bcm.pack(*fields)) == fields

    class Double:
        def __float__(self):
            return 1.5

    class FloatSubclass(float):
        def __float__(self):
            raise AssertionError("converted a float subclass")

    for prefix in ("", "@", "=", "<", ">", "!"):
        layout = struct.Struct(prefix + "2id")
        values = (-(1 << 31), (1 << 31) - 1, 1.5)
        assert layout.size == 16
        assert struct.unpack(layout.format, layout.pack(*values)) == values
        assert struct.unpack(prefix + "d", struct.pack(prefix + "d", Field(-7))) == (-7.0,)
        for value in (1 << 31, -(1 << 31) - 1, OverflowingIndex()):
            try:
                struct.pack(prefix + "i", value)
            except struct.error as error:
                assert str(error) == "'i' format requires -2147483648 <= number <= 2147483647"
            else:
                raise AssertionError("signed int overflow accepted")

    sample = struct.Struct(">f")
    assert sample.pack(Double()) == b"\x3f\xc0\x00\x00"
    assert struct.unpack_from("<f", b"x\x00\x00\xc0\x3f", 1) == (1.5,)
    assert struct.pack("!f", float("-inf")) == b"\xff\x80\x00\x00"
    assert struct.calcsize("@B0f") == struct.calcsize("@Bf") - 4
    assert struct.calcsize("=Bf") == 5
    signaling_nan = b"\x7f\xa0\x00\x01"
    assert sample.pack(sample.unpack(signaling_nan)[0]) == signaling_nan
    try:
        sample.pack(1e100)
    except OverflowError as error:
        assert str(error) == "float too large to pack with f format"
    else:
        raise AssertionError("finite float overflow accepted")

    for integer in (1 << 128, -(1 << 128)):
        try:
            sample.pack(integer)
        except struct.error as error:
            assert str(error) == "int too large to convert"
        else:
            raise AssertionError("integer overflow accepted")

    assert struct.calcsize("@B0d") == struct.calcsize("@Bd") - 8
    assert struct.calcsize("=Bd") == 9
    assert struct.pack("<i", Field(-3)) == b"\xfd\xff\xff\xff"
    assert struct.unpack(">i", b"\x80\x00\x00\x01") == (-(1 << 31) + 1,)
    assert struct.pack(">d", Double()) == b"\x3f\xf8" + bytes(6)
    assert struct.pack("<d", FloatSubclass(1.5)) == bytes(6) + b"\xf8\x3f"
    for raw in (b"\x80" + bytes(7), b"\x7f\xf0" + bytes(6), b"\x7f\xf8" + bytes(5) + b"\x04"):
        value, = struct.unpack("!d", raw)
        assert type(value) is float and struct.pack("<d", value) == raw[::-1]
    for value in ("1", 1 << 9999, OverflowingIndex()):
        try:
            struct.pack(">d", value)
        except struct.error as error:
            assert str(error) == "required argument is not a float"
        else:
            raise AssertionError("invalid double accepted")

    from operator import length_hint

    data = bytearray(b"\x00\x01\x00\x02")
    events = []

    class Source:
        def __buffer__(self, flags):
            events.append(flags)
            self.view = memoryview(data)
            return self.view

        def __release_buffer__(self, view):
            assert view is self.view
            view.release()
            events.append("released")

    source = Source()
    rows = struct.Struct(">H").iter_unpack(source)
    assert iter(rows) is rows and length_hint(rows) == 2
    assert next(rows) == (1,)
    data[3] = 7
    assert next(rows) == (7,) and rows.__length_hint__() == 0
    try:
        source.view.release()
    except BufferError:
        pass
    else:
        raise AssertionError("iterator released the view before exhaustion")
    assert next(rows, None) is None and next(rows, None) is None
    assert events == [0, "released"]
    data.append(0)
    try:
        struct.iter_unpack(">H", source)
    except struct.error as caught:
        assert caught.args == ("iterative unpacking requires a buffer of a multiple of 2 bytes",)
    else:
        raise AssertionError("partial record was accepted")
    assert events == [0, "released", 0, "released"]
