class Name:
    def __get__(self, obj, owner):
        return owner.__name__


class A:
    label = Name()

    def walk(self):
        return "A"


class B(A):
    def walk(self):
        return super().walk() + "B"


class C(A):
    def walk(self):
        return super().walk() + "C"


class D(C, B):
    def walk(self):
        return super().walk() + "D"

    def saved(self):
        return super()

    def enclosing(self):
        def nested(receiver):
            return __class__ is D, super().walk()
        return nested(self)

    def deleted(self):
        try:
            raise ValueError()
        except ValueError as self:
            pass
        return super()


class Proxy:
    @property
    def __class__(self):
        return D


def outside(value):
    return super()


class Mapping(dict):
    def __init__(self, pairs):
        super().__init__(pairs)

    def write(self, name, value):
        super().__setitem__(name, value)


def current():
    obj = D()
    obj.label = "instance"
    print(obj.walk(), D.walk(obj), obj.enclosing())
    proxy = obj.saved()
    for _ in range(6000):
        []
    print(proxy.walk(), obj.label, proxy.label, proxy.__thisclass__ is D, proxy.__self__ is obj)
    print(type(proxy) is super, super(int, bool).__self_class__ is bool, super(int, True).__self_class__ is bool)
    print(proxy.__self_class__ is D, super(C, D).walk(obj), super(C).__self__ is None)
    mapping = Mapping([("visits", 1)])
    mapping.write("visits", mapping["visits"] + 1)
    print(mapping["visits"], len(mapping))
    wrapped = Proxy()
    print(super(D, wrapped).__self__ is wrapped, super(D, wrapped).label)
    for call in (lambda: outside(obj), obj.deleted, lambda: super(C, A())):
        try:
            call()
        except (RuntimeError, TypeError) as error:
            print(str(error))
    try:
        class Duplicate(A, A):
            pass
    except TypeError as error:
        print(str(error))
    try:
        class Inconsistent(A, D):
            pass
    except TypeError as error:
        print(str(error))
