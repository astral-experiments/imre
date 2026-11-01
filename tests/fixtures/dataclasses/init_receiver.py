from dataclasses import dataclass, field
from inspect import signature
from typing import List


@dataclass
class Row:
    numbers: List[int] = field(default_factory=list)


@dataclass
class Named:
    self: int


for cls, name in ((Row, "self"), (Named, "__dataclass_self__")):
    assert next(iter(signature(cls.__init__).parameters)) == name
    for keywords, text in (({"unknown": 1, name: 2}, "unexpected keyword argument 'unknown'"),
                           ({name: 2, "unknown": 1}, "multiple values for argument " + repr(name))):
        try:
            cls(**keywords)
        except TypeError as exc:
            assert text in str(exc)
        else:
            raise AssertionError("accepted invalid constructor keywords")

a, b = Row(), Row()
Row.__init__(a, numbers=[4])
assert a.numbers == [4] and b.numbers == []
m = Named(self=1)
Named.__init__(m, self=2)
assert m.self == 2

try:
    @dataclass
    class Duplicate:
        self: int
        __dataclass_self__: int
except SyntaxError:
    pass
else:
    raise AssertionError("generated an initializer with two receiver parameters")
