import dataclasses
import gc
import types


def make():
    def read():
        return alpha, beta
    alpha = 1
    beta = 2
    return read


def paired_readers(value):
    return (lambda: value), (lambda: value)


def constant_factory():
    def constant():
        return 99
    return constant


def keep_function(_):
    return lambda func: func


def decorated_factory(value):
    @keep_function(captured := value)
    def read():
        return str(captured)

    return read


def class_closure(label):
    class Base:
        def label(self):
            return "base"

    class Owner(Base):
        def identify(self):
            return __class__, label

        def label(self):
            return "owner"

        def inherited(self):
            return super().label()

        def explicit(self, __class__):
            return __class__

        def reader(self):
            def named(receiver):
                yield super().label(), label
            return named

    class Derived(Owner):
        pass

    identify = Owner.identify
    cell, other = identify.__closure__
    assert identify.__code__.co_freevars == ("__class__", "label")
    assert (cell.cell_contents, other.cell_contents) == identify(None) == (Owner, label)
    assert Owner.inherited.__closure__[0] is cell
    assert Owner.explicit.__closure__ is None
    named = Owner().reader()
    suspended = named(Derived())
    assert suspended.gi_code is named.__code__
    assert suspended.gi_code.co_freevars == ("__class__", "label")
    assert named.__closure__[0] is cell and named.__closure__[1] is other
    assert next(suspended) == ("base", label)
    # Reconstruct the method with a different starting point for super().
    def replacement():
        return Derived

    copied = types.FunctionType(Owner.inherited.__code__, globals(), closure=replacement.__closure__)
    assert copied.__closure__[0] is replacement.__closure__[0]
    assert (Owner.inherited(Derived()), copied(Derived())) == ("base", "owner")
    return cell


def slotted_class_cells():
    class Foreign:
        def owner(self):
            return __class__

    class Record:
        foreign = Foreign.owner

        def owners(self):
            return __class__, payload

    # Both cells start with the old class. Only the declaring-class cell moves.
    payload = Record
    assert Record.owners.__code__.co_freevars == ("__class__", "payload")
    owner, other = Record.owners.__closure__
    foreign, = Foreign.owner.__closure__
    replacement = dataclasses.dataclass(slots=True)(Record)
    assert replacement.owners.__closure__[0] is owner
    assert replacement.owners.__closure__[1] is other
    assert (owner.cell_contents, other.cell_contents) == (replacement, Record)
    assert replacement().owners() == (replacement, Record)
    assert replacement.foreign.__closure__[0] is foreign
    assert replacement().foreign() is foreign.cell_contents is Foreign


def current():
    assert class_closure("first") is not class_closure("second")
    slotted_class_cells()
    decorated = decorated_factory(7)
    assert decorated.__code__.co_freevars == ("captured",)
    assert decorated.__closure__[0].cell_contents == 7
    assert decorated() == "7"

    name = "enclosing"

    class Owner:
        name = "class"
        source = [name]
        pairs = [(name, item) for item in source]
        readers = [lambda: name for item in source]

    assert Owner.pairs == [("enclosing", "class")]
    assert Owner.readers[0]() == "enclosing"

    __class__ = "enclosing"
    try:
        class Unfinished:
            values = [__class__ for _ in [0]]
    except NameError:
        pass
    else:
        raise AssertionError("captured the enclosing function's __class__")

    value = 1
    for read in (lambda: [value for _ in [0] if value for value in [2]],
                 lambda: next(value for _ in [0] if value for value in [2])):
        try:
            read()
        except UnboundLocalError:
            pass
        else:
            raise AssertionError("read a comprehension target before assignment")
    assert [value for value in [value]] == [1]

    fn = make()
    print(fn.__code__.co_freevars, fn(), tuple(cell.cell_contents for cell in fn.__closure__))
    copied = types.FunctionType(fn.__code__, {}, closure=fn.__closure__[::-1])
    print(copied(), copied.__closure__[0] is fn.__closure__[1])
    fn.__closure__[0].cell_contents = 8
    print(fn(), copied())
    empty = constant_factory()
    print(empty.__code__.co_freevars, empty.__closure__)
    print(types.FunctionType(empty.__code__, {})())
    print(types.FunctionType(empty.__code__, {}, closure=()).__closure__)

    left, right = paired_readers("before")
    gc.collect()
    left_cells = left.__closure__
    gc.collect()
    right_cells = right.__closure__
    assert left.__closure__ is left_cells
    assert right.__closure__ is right_cells
    assert left_cells is not right_cells
    assert left_cells[0] is right_cells[0]
    left_cells[0].cell_contents = "after"
    assert left() == right() == "after"

    supplied = (left_cells[0],)
    copied = types.FunctionType(left.__code__, {}, closure=supplied)
    assert copied.__closure__ is supplied
    assert copied() == "after"
