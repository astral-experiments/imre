import sys
sys.path.append("unused")


class MissingBoth:
    pass


class EnterOnly:
    def __enter__(self):
        return self


shape = int(sys.argv[1])
if shape == 0:
    manager = 1
elif shape == 1:
    manager = None
elif shape == 2:
    manager = []
elif shape == 3:
    manager = "text"
elif shape == 4:
    manager = b"data"
elif shape == 5:
    manager = (1,)
elif shape == 6:
    manager = MissingBoth()
elif shape == 7:
    manager = EnterOnly()
else:
    manager = open(sys.argv[2], "wb" if shape == 8 else "w")
try:
    with manager as stream:
        print("written", stream.write(b"safe" if shape == 8 else "safe"))
except MemoryError:
    print("kind MemoryError")
except TypeError as error:
    print("kind TypeError")
    print("detail", str(error))

if shape >= 8:
    print("closed", manager.closed)
    reader = open(sys.argv[2], "rb")
    print("content", reader.read() == b"safe")
    reader.close()
