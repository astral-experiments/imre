import types


def make():
    value = 7

    def read():
        return value

    def write(new):
        nonlocal value
        value = new

    return read, write


def current():
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
