from operator import itemgetter


def getter_repr():
    visits = []

    class Key:
        def __init__(self, name):
            self.name = name

        def __repr__(self):
            visits.append(self.name)
            if self.name == "raise":
                raise LookupError("key repr")
            if self.name == "invalid":
                return None
            if self.name == "cycle":
                return repr(cycle)
            return self.name

    print(repr([itemgetter((Key("a"),), Key("b"))]))
    for name in ("raise", "invalid"):
        try:
            print(str(itemgetter(Key(name), Key("unvisited"))))
        except (LookupError, TypeError) as error:
            print(type(error).__name__, error.args)
    cycle = itemgetter(Key("cycle"))
    print(repr(cycle), visits)


class Lookup:
    def __init__(self):
        self.keys = []
        self.saved = ["first"]

    def __getitem__(self, key):
        self.keys.append(key)
        if key == "saved":
            return self.saved
        if key == "replace":
            self.saved = None
            for i in range(2200):
                _ = [i, {}]
        if key == "missing":
            raise KeyError(key)
        return key


def current():
    getter_repr()
    column = itemgetter(1)
    rows = [("b", 2), ("a", 1), ("c", 2)]
    sorts = ["endpoint", "methods"]
    rows.sort(key=itemgetter(sorts.index("methods")))
    print(rows, callable(column), isinstance(column, itemgetter))
    print(itemgetter(-1, 0)("aé"), itemgetter(slice(1, 3))((1, 2, 3)))
    print(itemgetter("right", "left")({"left": 1, "right": 2}))
    print(repr(itemgetter("right", -1)))
    for getter in (itemgetter((1, 2)), itemgetter(1, 2)):
        factory, keys = getter.__reduce__()
        print(type(getter) is itemgetter, factory is itemgetter, keys, keys is getter.__reduce__()[1])

    class AppendIndex:
        def __index__(self):
            rows.append(("new", 3))
            return -1

    print(itemgetter(AppendIndex())(rows))
    print(itemgetter(-1)(range(0, 10**12)), itemgetter(0)(b"x"))
    obj = Lookup()
    key = ["retained"]
    get_key = itemgetter(key)
    key.append(2)
    print(get_key(obj) is key)
    print(itemgetter("saved", "replace", "replace")(obj))
    for call in (
        lambda: itemgetter(),
        lambda: itemgetter(item=0),
        lambda: column(),
        lambda: column(rows, rows),
        lambda: column(rows, size=1),
        lambda: itemgetter("missing")({}),
        lambda: itemgetter("before", "missing", "after")(obj),
        lambda: itemgetter(9)(rows),
        lambda: itemgetter("key")("text"),
        lambda: itemgetter(1 << 100)(rows),
        lambda: itemgetter(1 << 100)(range(3)),
        lambda: itemgetter(0)(iter(rows)),
    ):
        try:
            call()
        except (TypeError, IndexError, KeyError) as exc:
            print(type(exc).__name__, str(exc))
    print(obj.keys)


if __name__ == "__main__":
    current()
