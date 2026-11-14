class Target:
    def __init__(self, name):
        self.name = name

    def __delitem__(self, key):
        print("delete", self.name)
        return Target("unused result")

    def __del__(self):
        print("release", self.name)


def following(fail):
    print("next receiver")
    if fail:
        raise LookupError
    return Target("second")


def current():
    for fail in (False, True):
        try:
            del Target("first")[Target("left"), Target("right")], following(fail)[Target("last")]
        except LookupError:
            print("caught")

    remaining = Target("remaining local")
    first = Target("previous first")
    second = Target("previous second")
    first = Target("first local")
    second = Target("second local")
    print("bound", first.name, second.name)
    del second, first
