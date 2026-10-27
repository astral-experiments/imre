from collections.abc import Collection, Mapping, MutableMapping, MutableSet, Set


class Store(MutableMapping[str, int]):
    def __init__(self):
        self.data = {}

    def __getitem__(self, key):
        return self.data[key]

    def __setitem__(self, key, value):
        self.data[key] = value

    def __delitem__(self, key):
        del self.data[key]

    def __len__(self):
        return len(self.data)

    def __iter__(self):
        return iter(self.data)


class DictChild(dict):
    pass


def current():
    print(MutableMapping.__bases__ == (Mapping,), Store.__bases__ == (MutableMapping,))
    print(Store.__orig_bases__[0].__origin__ is MutableMapping)
    for value in ({}, DictChild(), [("Host", "localhost")], Store()):
        print(isinstance(value, Mapping), isinstance(value, MutableMapping))
    for cls in (DictChild, Store, list):
        print(issubclass(cls, Mapping), issubclass(cls, MutableMapping))
    try:
        MutableMapping()
    except TypeError as error:
        print(str(error))
    data = Store()
    shared = []
    print(data.get("missing", shared) is shared, data.setdefault("first", shared) is shared)
    view = data.keys()
    data.update({"second": 2}, third=3)
    print(len(view), list(view), data.get("second"), data.pop("missing", shared) is shared)
    print(data.pop("second"), list(data.items()))
    data.clear()
    print(len(data), list(view))

    data.update({"a": 1, "b": 2})
    values = iter(data.values())
    print(next(values))
    data["b"] = 3
    print(next(values))


    class Registered:
        pass

    print(MutableSet.register(Registered) is Registered)
    print(isinstance(Registered(), MutableSet), issubclass(Registered, Collection))
    print(issubclass(Registered, Mapping), MutableSet not in Registered.__mro__)
    for value in ({}, set(), frozenset()):
        print(isinstance(value, Collection), isinstance(value, Set), isinstance(value, MutableSet))
