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
