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
