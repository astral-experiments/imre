"""Compare item writes through an inherited descriptor with builtin containers."""


def current():
    events = []

    class SetItem:
        def __get__(self, obj, owner):
            if obj is None:
                return self
            events.append("bind")
            def write(key, value):
                if key == "blocked":
                    raise LookupError(key)
                obj.data[key] = value
                return "ignored"
            return write

    class Mapping:
        __setitem__ = SetItem()

    class Child(Mapping):
        pass

    mapping = Child()
    mapping.data = {}
    mapping.__setitem__ = lambda key, value: events.append("shadow")
    mapping["visits"] = 1
    mapping["visits"] = 2
    print(mapping.data, events)
    mapping.__setitem__("visits", 3)
    try:
        mapping["blocked"] = 4
    except LookupError as error:
        print(error.args, mapping.data, events)

    class Missing:
        def __getattr__(self, name):
            raise AssertionError("instance fallback should not run")

    class Uncallable:
        __setitem__ = None

    for value in (Missing(), Uncallable(), (1,)):
        try:
            value["new"] = 0
        except TypeError as error:
            print(error.args)

    sequence = [0, 1]
    sequence[-1] = 7
    print(sequence)
