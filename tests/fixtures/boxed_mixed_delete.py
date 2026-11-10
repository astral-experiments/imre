shared = 1


class Box:
    pass


def mark(label, value):
    print(label)
    return value


def current():
    global shared
    shared = 1
    cell = 2
    box = Box()
    box.x = 3
    items = {"keep": 4}

    def erase():
        global shared
        nonlocal cell
        del (
            mark("box", box).x,
            [cell, shared],
            mark("items", items)[mark("key", "absent")],
            mark("late", items)["keep"],
        )

    try:
        erase()
    except KeyError as error:
        print("KeyError", error.args[0])
    try:
        print(cell)
    except UnboundLocalError:
        print("cell empty")
    print("shared" in globals(), hasattr(box, "x"), items["keep"])
