import sys
sys.path.append("empty")
from _weakref import proxy

class Values(list):
    pass

owner = Values([41, 42])
forwarded = proxy(owner)
count = int(sys.argv[1])
marker = "arm weak proxy root refusal"
kind = "ok"
try:
    result = [str(i) + str(forwarded[0]) for i in range(count)]
except MemoryError:
    kind = "MemoryError"
print("kind", kind)
print("preserved", owner == [41, 42])
print("retry", forwarded[0] == 41)
