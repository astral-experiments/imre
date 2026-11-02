import types
from types import CellType
from _weakref import ref

marker = "module"


class Payload:
    pass


def global_lambda(unused):
    return (lambda: marker), ref(unused)


def decorated_reader(captured):
    def keep(fn):
        return fn

    @keep
    def read(key=marker, /, *, count=0):
        return captured, marker, key, count

    return read


def make_late():
    def read():
        return payload, marker

    def write(value):
        nonlocal payload
        payload = value

    def factorial(n):
        if n < 2:
            return 1
        return n * factorial(n - 1)

    payload = 17
    return read, write, factorial


def globally_bound_reader():
    marker = "outer"

    def enclosing():
        global marker

        def read():
            return marker

        return read

    return enclosing()


def make():
    value = 7

    def read():
        return value

    def write(new):
        nonlocal value
        value = new

    return read, write


def current():
    assert CellType is types.CellType
    empty = CellType()
    assert type(empty) is CellType
    try:
        empty.cell_contents
    except ValueError as error:
        assert error.args == ("Cell is empty",)
    else:
        raise AssertionError("empty cell had a value")

    empty.cell_contents = None
    assert empty.cell_contents is None
    payload = object()
    cell = CellType.__new__(CellType, payload)
    assert cell.cell_contents is payload
    assert CellType(payload) is not cell

    read, write = make()
    code = read.__code__
    cell = read.__closure__[0]
    copied = types.FunctionType(code, {}, closure=read.__closure__)
    print(code.co_freevars, cell.cell_contents, copied())
    print(copied.__closure__[0] is cell)
    write(23)
    print(read(), copied(), cell.cell_contents)
    cell.cell_contents = 41
    print(read(), copied(), cell.cell_contents)
    for closure in (None, (), (1,)):
        try:
            types.FunctionType(code, {}, closure=closure)
        except Exception as e:
            print(type(e).__name__, e.args)
    del cell.cell_contents
    try:
        print(cell.cell_contents)
    except ValueError:
        print("empty")
    cell.cell_contents = 52
    print(read(), copied())
    print(type(cell).__name__)

    read, write, factorial = make_late()
    cell = read.__closure__[0]
    supplied = {"marker": "replacement"}
    copied = types.FunctionType(read.__code__, supplied, closure=(cell,))
    print(cell is write.__closure__[0], cell is copied.__closure__[0])
    print(read.__globals__ is globals(), copied.__globals__ is supplied)
    print(read(), copied())
    write(29)
    print(cell.cell_contents, read(), copied())
    print(factorial(5), factorial.__closure__[0].cell_contents is factorial)

    reader = globally_bound_reader()
    print(reader.__code__.co_freevars, reader.__closure__, reader())
    print(types.FunctionType(reader.__code__, {"marker": "supplied"})())

    reader = decorated_reader(8)
    assert reader.__code__.co_freevars == ("captured",)
    replay = types.FunctionType(reader.__code__, {"marker": "replacement"},
                                argdefs=reader.__defaults__, closure=reader.__closure__)
    replay.__kwdefaults__ = reader.__kwdefaults__
    assert replay() == (8, "replacement", "module", 0)

    reader, unused = global_lambda(Payload())
    pressure = []
    for index in range(20000):
        pressure.append([index])
    print(reader(), unused() is None)
