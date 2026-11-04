import codecs


class Count:
    def __index__(self) -> int:
        return 1


entry = codecs.lookup("idna")
for source in (bytearray(b"x.y"), memoryview(b"xn--bcher-kva"), Count(), object()):
    try:
        print(entry.decode(source))
    except TypeError as error:
        print(error.args)

for size in (1, Count()):
    first = bytes(size)
    second = bytes(size)
    print(first, first is second)
