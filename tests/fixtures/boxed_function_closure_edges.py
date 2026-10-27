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
