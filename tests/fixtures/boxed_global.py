state = None


def outer():
    state = "enclosing"

    def install():
        if False:
            global state

        def read():
            return state

        state = ["installed"]
        return read

    return install(), state


def clear():
    global state
    del state


def current():
    reader, enclosing = outer()
    print(enclosing, reader() is state, state)
    clear()
    try:
        reader()
    except NameError:
        print("removed")
    for fn, args in ((read_local, (False,)), (read_cell, (False, False)), (read_cell, (False, True))):
        try:
            print(fn(*args))
        except NameError as error:
            print(type(error).__name__, str(error))
    print(read_class_namespace(False), read_class_namespace(True), read_class_cell(True))
    try:
        print(read_class_cell(False))
    except NameError as error:
        print(str(error))


marker = "global"


def read_class_namespace(assigned):
    if assigned:
        marker = "outer"

    class C:
        seen = marker
        marker = "class"
    return C.seen


def read_class_cell(assigned):
    if assigned:
        marker = "outer"

    class C:
        seen = marker
    return C.seen


def read_local(assigned):
    if assigned:
        marker = "local"
    return marker


def read_cell(assigned, child):
    def read():
        return marker

    if assigned:
        marker = "local"
    return read() if child else marker
