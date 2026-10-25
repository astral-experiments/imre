"""Raise a real CPython allocator exception during an aggregate comparison on Linux."""
from dataclasses import dataclass
import resource
import sys


class Operand:
    def __eq__(self, other):
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


wrap = Frame if sys.argv[1] == "direct" else lambda item: (item,)
try:
    print(wrap(Operand()) == wrap(Operand()))
except MemoryError as error:
    print(repr(error) + "|" + str(error))
