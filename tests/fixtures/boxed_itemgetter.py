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
    pair = itemgetter(Key("cycle"), 7)
    cycle = pair.__reduce__()[1]
    print(repr(pair), visits)
    cycle = pair
    print(repr(pair.__reduce__()[1]), visits)


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


def index_lifetime():
    from _weakref import ref

    class Element:
        def __init__(self, remove):
            self.remove = remove

        def __eq__(self, other):
            if self.remove:
                del items[0]
                return False
            for i in range(5500):
                _ = [i, {}]
            return watched() is None

    items = [Element(True), None, Element(False)]
    watched = ref(items[0])
    print(items.index(None))


def current():
    index_lifetime()
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


def constructor_allocation():
    keys = (0,) * 113
    marker = {}
    marker["probe"] = True
    try:
        getter = itemgetter(*keys)
    except MemoryError as error:
        print("caught", error.args)
        getter = itemgetter(*keys)
    print(len(getter.__reduce__()[1]))


def result_allocation(kind):
    from operator import attrgetter

    class Target:
        def __getitem__(self, key):
            visits.append(key)
            return key

        @property
        def field(self):
            visits.append("field")
            return "field"

    visits = []
    target = Target()
    getter = itemgetter(*((0,) * 113)) if kind == "item" else attrgetter(*(("field",) * 113))
    marker = {}
    marker["probe"] = True
    try:
        result = getter(target)
    except MemoryError as error:
        print("caught", error.args, len(visits))
        result = getter(target)
    print(len(result), len(visits))


if __name__ == "__main__":
    current()
