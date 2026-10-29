import struct


def current():
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

    for fmt in (Text(">Q"), Data(b">Q")):
        record = struct.Struct(fmt)
        print(record.format, struct.calcsize(fmt))
        for raw in (bytearray(8), Data(bytes(8)), Buffer(bytes(8))):
            print(record.unpack(raw), struct.unpack(fmt, raw))
        try:
            struct.unpack(fmt, Buffer(b"short"))
        except struct.error as error:
            print(str(error))
