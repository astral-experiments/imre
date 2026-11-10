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

    # The first target can release the only mapping reference to the second item.
    import gc

    class Field:
        label = "retained"

    entries = {"key": Field()}

    class Targets:
        def __setitem__(self, index, value):
            if index == 0:
                entries.clear()
                gc.collect()
                print(value)
            else:
                print(value.label)

    targets = Targets()
    for targets[0], targets[1] in entries.items():
        break

    def mark(label, value):
        events.append(label)
        return value

    class Descriptor:
        def __set__(self, obj, value):
            events.append(("descriptor", value))
            obj.saved = value

    class Record:
        field = Descriptor()

    record = Record()
    events.clear()

    class Assignment:
        mark("single receiver", record).field = mark("single RHS", 3)
        try:
            whole = [
                mark("receiver", record).field,
                (
                    mark("first item", mapping)[mark("first key", "ok")],
                    mark("next item", mapping)[mark("next key", "blocked")],
                ),
            ] = mark("chain RHS", (4, (5, 6)))
        except LookupError:
            pass

    print(Assignment.whole, record.saved, mapping.data["ok"], events)

    def unpack(rows):
        whole = mark("receiver", record).field, (mark("table", mapping)[mark("key", "ok")], last) = (
            value for value in mark("rhs", rows)
        )
        return list(whole), last

    events.clear()
    print(unpack((10, (11, 12))), record.saved, mapping.data["ok"], events)
    events.clear()
    try:
        unpack((20, (21,)))
    except ValueError as error:
        print(str(error), record.saved, mapping.data["ok"], events)
