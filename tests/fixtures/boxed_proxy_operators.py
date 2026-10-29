"""First-class operators retained by Werkzeug's LocalProxy descriptors."""

import operator as op
from functools import partial
from operator import setitem


def current():
    class Unsized:
        pass

    class Number:
        def __index__(self):
            return 5

        def __length_hint__(self):
            return 7

        def __matmul__(self, other):
            return "forward"

        def __imatmul__(self, other):
            return "in-place"

    class Refuse:
        def __matmul__(self, other):
            return NotImplemented

        def __imatmul__(self, other):
            return NotImplemented

    class Right(Number):
        def __rmatmul__(self, other):
            return "reflected"

    class Mapping(dict):
        def __getitem__(self, key):
            events.append(("get", key))
            return dict.__getitem__(self, key)

        def __contains__(self, key):
            events.append(("contains", key))
            return dict.__contains__(self, key)

    values = [4, 2]
    alias = values
    alias *= 0
    print(alias is values, values)
    alias += [7]
    print(alias is values, values)

    events = []
    mapping = Mapping(visits=2)
    print(op.getitem(mapping, "visits"), op.contains(mapping, "absent"), events)

    comparisons = (op.lt, op.le, op.eq, op.ne, op.gt, op.ge)
    print([f(9, 4) for f in comparisons])
    operations = (op.add, op.sub, op.mul, op.truediv, op.floordiv, op.mod,
                  op.lshift, op.rshift, op.and_, op.xor, op.or_)
    print([f(9, 2) for f in operations])
    inplace = (op.iadd, op.isub, op.imul, op.itruediv, op.ifloordiv, op.imod,
               op.ilshift, op.irshift, op.iand, op.ixor, op.ior, op.ipow)
    print([f(9, 2) for f in inplace])
    print(op.matmul(Number(), 1), op.matmul(Number(), Right()), op.imatmul(Number(), 1))
    print(op.matmul(Refuse(), Right()), op.imatmul(Refuse(), Right()))
    print(op.neg(9), op.pos(-9), op.invert(9), op.index(Number()))
    print(op.length_hint([9]), op.length_hint(Number()), op.length_hint(Unsized(), Number()))
    class Count(int):
        def __index__(self):
            raise AssertionError("use stored value")

    class Unavailable(TypeError):
        pass

    class Hinted:
        def __init__(self, value):
            self.value = value

        def __len__(self):
            raise Unavailable()

        def __length_hint__(self):
            if self.value is None:
                raise Unavailable()
            return self.value

    print(op.length_hint(Hinted(Count(3))), op.length_hint(Hinted(None), 9))
    for value in (Count(1 << 80), -(1 << 80)):
        try:
            op.length_hint(Hinted(value))
        except OverflowError as error:
            print(error.args)
    values = [1, 2]
    alias = values
    print(op.iadd(values, (3, 4)) is alias, op.imul(values, 2) is alias, values)
    print(op.contains(values, 3), op.getitem(values, -1))
    print(op.delitem(values, -1), values)
    print(setitem is op.setitem, hasattr(setitem, "__get__"))
    for count in (1, 2, 1, 1):
        session = {}
        write = partial(setitem, session)
        print(write("visits", count) is None, session["visits"])
        print(op.delitem(session, "visits"), session)
    for bad_call in (lambda: op.neg(), lambda: op.iadd(1), lambda: op.length_hint(),
                     lambda: setitem({}, "visits"), lambda: setitem({}, key="visits", value=1)):
        try:
            bad_call()
        except TypeError as error:
            print(str(error))


current()
