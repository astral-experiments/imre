import sys
sys.path.append("empty")
from _weakref import proxy

class Values(list):
    pass


def check(read_many):
    owner = Values([41, 42])
    forwarded = proxy(owner)
    marker = "arm weak proxy root refusal"
    kind = "ok"
    try:
        read_many(forwarded, 0)
    except MemoryError:
        kind = "MemoryError"
    print("kind", kind)
    print("preserved", owner == [41, 42])
    print("retry", forwarded[0] == 41)
