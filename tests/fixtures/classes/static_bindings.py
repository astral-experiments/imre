import hashlib
from typing import Callable


def plus(a: int, b: int = 3) -> int:
    return a + b


class Operations:
    digest = staticmethod(hashlib.sha256)
    calculate = staticmethod(plus)
    hashes = hashlib

    def run(self, data: bytes, n: int) -> str:
        value = self.calculate(n, b=4)
        print(value)
        return self.hashes.sha256(data).hexdigest()


class Derived(Operations):
    pass


def use(op: Operations) -> None:
    print(op.digest(b"a").hexdigest())
    binary: Callable[[int, int], int] = op.calculate
    print(binary(1, 9))
    print(op.run(b"b", 5))


print(Operations.calculate(1))
print(Derived.hashes.sha256(b"c").hexdigest())
use(Derived())
