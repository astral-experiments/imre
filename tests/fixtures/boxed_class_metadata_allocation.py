import sys
from collections import namedtuple

sys.path.append("unused")


class Plain:
    pass


class Slotted:
    __slots__ = ()


class Parent:
    pass


class Child(Parent):
    pass


class Initializer:
    value: int

    def __init__(self):
        self.value = 41


class Left(Parent):
    pass


class Right(Parent):
    pass


class Diamond(Left, Right):
    pass


class ExceptionChild(ValueError):
    pass


EmptyRecord = namedtuple("EmptyRecord", [])
classes = [Plain, Slotted, Child, Initializer, Diamond, EmptyRecord, ExceptionChild]
shape = int(sys.argv[1])
target = classes[shape]
name = target.__name__
inherits = issubclass(target, Parent)
marker = "arm class metadata refusal"
kind = "ok"
try:
    target()
except MemoryError:
    kind = "MemoryError"
print("kind", kind)
retry = target()
print("metadata", target.__name__ == name, issubclass(target, Parent) == inherits)
print("retry", type(retry).__name__, isinstance(retry, target))
if shape == 3:
    print("value", retry.value == 41)
if shape == 5:
    print("record", len(retry) == 0, retry._fields == ())
if shape == 6:
    print("exception", retry.args == ())
