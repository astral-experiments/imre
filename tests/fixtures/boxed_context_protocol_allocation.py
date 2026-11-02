import sys
sys.path.append("unused")


class ExitOnly:
    def __exit__(self, *exc):
        pass


class EnterOnly:
    def __enter__(self):
        return self


operation = sys.argv[1]
if operation == "lookup_enter":
    manager = ExitOnly()
elif operation == "lookup_exit":
    manager = EnterOnly()
else:
    manager = open(sys.argv[2], operation)
try:
    with manager as stream:
        print("written", stream.write(b"safe" if operation == "wb" else "safe"))
except MemoryError:
    print("kind MemoryError")
except TypeError as error:
    print("kind TypeError")
    print("detail", str(error))

if operation in ("w", "wb"):
    print("closed", manager.closed)
    reader = open(sys.argv[2], "rb")
    print("content", reader.read() == b"safe")
    reader.close()
