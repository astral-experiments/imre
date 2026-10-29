import types


def make():
    def read():
        return alpha, beta
    alpha = 1
    beta = 2
    return read


def constant_factory():
    def constant():
        return 99
    return constant


def current():
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
