import sys
from weakref import proxy

sys.path.append("unused")
shape = int(sys.argv[1])


class NoCall:
    def __init__(self, value):
        self.value = value


class DisabledCall(NoCall):
    __call__ = None


owner = NoCall(41)
targets = [None, 1, True, 1.5, "text" * 1024, b"text" * 1024, [1, 2], (1, 2), {"kept": 41}, {1, 2}, NoCall(41), ValueError(41), 1 << 2048, proxy(owner), bytearray(b"kept"), DisabledCall(41)]
target = targets[shape]
marker = "arm noncallable diagnostic refusal"
try:
    target()
except MemoryError:
    print("kind MemoryError")
except TypeError as error:
    print("kind TypeError")
    print(str(error))

print("preserved", targets[4] == "text" * 1024 and targets[5] == b"text" * 1024 and targets[6] == [1, 2] and targets[7] == (1, 2) and targets[8]["kept"] == 41 and targets[10].value == 41 and targets[11].args[0] == 41 and targets[12] == 1 << 2048 and targets[13].value == 41 and bytes(targets[14]) == b"kept" and targets[15].value == 41)
try:
    target()
except TypeError:
    print("retry True")
