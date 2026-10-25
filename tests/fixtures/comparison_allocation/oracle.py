"""Raise a real CPython allocator exception during an aggregate comparison on Linux."""
from collections import OrderedDict
from dataclasses import dataclass
import resource
import sys


class Operand:
    refuse = True

    def __hash__(self):
        return 1

    def __eq__(self, other):
        if not Operand.refuse:
            return True
        Operand.refuse = False
        old = resource.getrlimit(resource.RLIMIT_AS)
        cap = min(n for n in (*old, 64 << 20) if n != resource.RLIM_INFINITY)
        resource.setrlimit(resource.RLIMIT_AS, (cap, old[1]))
        try:
            bytearray(128 << 20)
        finally:
            resource.setrlimit(resource.RLIMIT_AS, old)
        raise AssertionError("allocation exceeded the process address-space limit")


@dataclass(slots=True)
class Frame:
    field: object


mode = sys.argv[1]
if mode in ("ordered", "plain"):
    left_key, right_key = ((Operand(), 4), 3), ((Operand(), 4), 3)
    left = OrderedDict([(left_key, 7)])
    right = (OrderedDict if mode == "ordered" else dict)([(right_key, 7)])
else:
    wrap = Frame if mode == "direct" else lambda item: (item,)
    left, right = wrap(Operand()), wrap(Operand())
try:
    print(left == right)
except MemoryError as error:
    print(repr(error) + "|" + str(error))
if mode in ("ordered", "plain"):
    print(len(left))
    print(len(right))
    print(left[left_key])
    print(right[right_key])
    print(left == right)
