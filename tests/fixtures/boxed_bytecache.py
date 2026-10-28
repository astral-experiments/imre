def current():
    import marshal
    import pickle
    import sys

    version = (sys.version_info[0] << 24) | sys.version_info[1]
    magic = b"j2" + pickle.dumps(5, 2) + pickle.dumps(version, 2)
    print(version, list(magic))
    for number in (False, True, -1, 255, 256, 65535, 65536, -2147483648, -2147483649, 2147483648, 1 << 75):
        print(list(pickle.dumps(number, protocol=2)))
    wide = pickle.dumps(1 << 2048, 2)
    print(len(wide), list(wide[:7]), list(wide[-3:]))

    # Independent marshal records from CPython, including a reference-flagged integer.
    print(marshal.loads(b"\xe9\x0e\x00\x00\x03trailing"), marshal.loads(b"i\xff\xff\xff\xff"))
    print(marshal.loads(b"\xf3\x03\x00\x00\x00j2\x00trailing") == b"j2\x00")
    print(marshal.loads(bytearray(b"i\xff\xff\xff\xff")))
    class Record(bytes):
        def __bytes__(self):
            raise AssertionError("marshal must read the buffer directly")

    print(marshal.loads(Record(b"i\xff\xff\xff\xff")))
    print(marshal.loads(b"\xec\x03\x00\x00\x00\x00\x00\x00\x00\x02\x00"))
    print(marshal.loads(b"l\xfd\xff\xff\xff\x01\x00\x00\x00\x02\x00trailing"))
    print(marshal.loads(b"N"), marshal.loads(b"T"), marshal.loads(b"F"))
    for data in (b"", b"i\x01", b"l\x02\x00\x00\x00\x00\x00\x00\x00", b"s\x03\x00\x00\x00x", b"s\xff\xff\xff\xff", b"?", b"0", 3, b"r\x00\x00\x00\x00", b"c"):
        try:
            marshal.loads(data, allow_code=False)
        except (EOFError, ValueError, TypeError) as error:
            print(type(error).__name__, str(error))

    class Protocol:
        def __index__(self):
            print("protocol index")
            return 2

    class Flag:
        def __bool__(self):
            print("code policy")
            raise ValueError("flag")

    print(list(pickle.dumps(obj=5, protocol=Protocol())))
    for data in (3, b"N", Record(b"N")):
        try:
            marshal.loads(data, allow_code=Flag())
        except (ValueError, TypeError) as error:
            print(type(error).__name__, str(error))


    data = bytearray(b"N")

    class ResizingPolicy:
        def __bool__(self):
            data.extend(b"x")
            return True

    try:
        marshal.loads(data, allow_code=ResizingPolicy())
    except BufferError as error:
        print(type(error).__name__, str(error), data)

    data = bytearray(b"F")

    class ReplacingPolicy:
        def __bool__(self):
            data[0] = 84
            return True

    print(marshal.loads(data, allow_code=ReplacingPolicy()), data)
