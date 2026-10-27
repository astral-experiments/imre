"""Compare native property assignment, accessor copies, and descriptor precedence."""


class Cell:
    def __init__(self):
        self.saved = 1

    @property
    def first(self):
        return self.saved

    @first.setter
    def value(self, n):
        if n < 0:
            raise ValueError(n)
        self.saved = n
        return "ignored"


class Child(Cell):
    pass


def current():
    from io import RawIOBase

    child = Child()
    print(isinstance(Cell.value, property), Cell.first is Cell.value, Child.value is Cell.value)
    print(Cell.value.fget is Cell.first.fget, Cell.first.fset is None, Cell.value.fdel)
    copy = property.setter(Cell.value, None)
    print(copy is Cell.value, copy.fget is Cell.value.fget, copy.fset is Cell.value.fset)
    object.__setattr__(child, "value", 3)
    print(child.value, child.first)
    try:
        child.value = -1
    except ValueError as exc:
        print(exc.args, child.first)
    try:
        child.first = 4
    except AttributeError as exc:
        print(exc.args)

    for call in (
        lambda: copy.setter(),
        lambda: copy.setter(None, None),
        lambda: copy.setter(fset=None),
        lambda: property.setter(1, None),
        lambda: property.setter(),
    ):
        try:
            call()
        except TypeError as exc:
            print(exc.args)

    for name in ("fget", "fset", "fdel"):
        try:
            setattr(copy, name, None)
        except AttributeError as exc:
            print(exc.args)

    class Wrong:
        field = copy.setter(5)

    try:
        Wrong().field = 5
    except TypeError as exc:
        print(exc.args)
    for i in range(6000):
        []
    child.value = 9
    print(child.first, copy.fset is Cell.value.fset)

    class Stream(RawIOBase):
        pass

    stream = Stream()
    print(stream.closed, callable(Stream.closed), hasattr(Stream.closed, "setter"))
    try:
        stream.closed = True
    except AttributeError as exc:
        print(exc.args)
    stream.close()
    print(stream.closed)

    class Parent(property):
        def __init__(self, fget, fset=None, fdel=None, doc=None):
            super().__init__(fget, fset, fdel, doc=doc)
            self.accessor = fget

    class Sub(Parent):
        pass

    class Owner:
        saved = 5

        @Sub
        def managed(self):
            "managed doc"
            return self.saved

        @managed.setter
        def managed(self, value):
            self.saved = value

    descriptor = Owner.managed
    obj = Owner()
    assert type(descriptor) is Sub and descriptor.fget is descriptor.accessor
    assert descriptor.__doc__ == "managed doc" and descriptor.__name__ == "managed"
    assert descriptor.__get__(None, Owner) is descriptor and obj.managed == 5
    obj.managed = 13
    assert obj.saved == 13 and descriptor.__get__(obj) == 13
    assert property.__init__(descriptor, child, doc="new doc") is None
    assert descriptor.fget is child and descriptor.fset is None and descriptor.__doc__ == "new doc"

    for call in (
        lambda: property.__init__(obj),
        lambda: property.__init__(descriptor, 1, fget=2),
        lambda: descriptor.__get__(None, None),
        lambda: descriptor.__set_name__(Owner),
        lambda: object.__init__(object(), 1),
    ):
        try:
            call()
        except TypeError as exc:
            print(exc.args)
