def current():
    class NestedDict(dict):
        pass
    class NestedList(list):
        pass
    class NestedTuple(tuple):
        pass
    mapping = sequence = items = None
    for _ in range(160):
        mapping = NestedDict({0: mapping})
        sequence = NestedList([sequence])
        items = NestedTuple((items,))
    assert [len(repr(value)) for value in (mapping, sequence, items)] == [804, 324, 484]

    items = []
    items.append(items)
    print(repr(items), str(items))

    mapping = {}
    mapping["again"] = mapping
    print(mapping)

    link = []
    record = (link,)
    link.append(record)
    print(record, link)

    child = ["child"]
    print([child, child])
    print([child, ValueError(child)])
    cross = []
    parent = {"list": cross}
    cross.append(parent)
    print(parent, cross)

    class Word(str):
        def __repr__(self):
            return "<word>"

    word = Word("text")
    print(repr([word, (word,), {word: word}]), [word])

    class Broken:
        def __repr__(self):
            raise LookupError("rendered")

    try:
        repr([Broken()])
    except LookupError as exc:
        print(exc.args)

    class Key:
        def __repr__(self):
            del mapping[self]
            return repr(mapping)

    key = Key()
    mapping = {key: "saved", "next": 2}
    print(repr(mapping))

    class Member:
        def __hash__(self):
            return 0

        def __repr__(self):
            group.add(4)
            return "member"

    group = {Member(), 1}
    print(repr(group), len(group))

    for kind in (list, tuple):
        class Derived(kind):
            def __repr__(self):
                return "override"

        values = Derived((1, 2))
        slot = kind.__repr__
        bound = slot.__get__(values, Derived)
        print(slot.__qualname__, slot.__objclass__ is kind, bound.__self__ is values)
        print(repr(values), slot(values), bound(), len(values), values[-1], list(values))
        plain = kind((1, 2))
        other = (tuple if kind is list else list)((1, 2))
        print(values == plain, plain == values, values == other)

        class NeverEqual(kind):
            def __eq__(self, other):
                return False

        print(plain == NeverEqual((1, 2)))
        print({slot: "registered"}[kind.__repr__])
        try:
            slot({})
        except TypeError as error:
            print(error)

    class Appending:
        def __repr__(self):
            mutable.append("new")
            return "appending"

    mutable = [Appending(), "old"]
    print(list.__repr__(mutable))
    cycle = []
    value = (cycle,)
    cycle.append(value)
    print(tuple.__repr__(value))


    for owner, raw in ((str, "a'b"), (bytes, b"a'b"), (bytearray, b"a'b")):
        class Scalar(owner):
            def __repr__(self):
                return "override"

        obj = Scalar(raw)
        slot = owner.__repr__
        bound = slot.__get__(obj, Scalar)
        assert slot.__objclass__ is owner and bound.__self__ is obj
        assert hash(slot) == object.__hash__(slot)
        assert {(slot,): 1}[(owner.__repr__,)] == 1
        print(slot.__qualname__, repr(obj), slot(obj), bound())


def allocation():
    value = [23]
    for _ in range(18):
        value = [value]
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        repr(value)
    except MemoryError as error:
        print("caught", error.args)
    print(len(repr(value)))


def defaultdict_allocation():
    from collections import defaultdict

    visits = []

    class Factory:
        def __call__(self):
            return None

        def __repr__(self):
            visits.append(1)
            return "the factory"

    value = defaultdict(Factory())
    value["entry"] = "q" * 91
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        text = repr(value)
    except MemoryError as error:
        print("caught", error.args, len(visits))
    else:
        print("value", len(text), len(visits))
    print("retry", len(repr(value)), len(visits))


def integer_allocation(wide):
    value = -((1 << 4097) - 37) if wide else 23
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        text = str(value)
    except MemoryError as error:
        print("caught", error.args)
    else:
        print("value", len(text), text[:5], text[-5:])
    text = str(value)
    print("retry", len(text), text[:5], text[-5:])


def integer_digit_limit(limit):
    over = 10 ** (limit or 700)
    for value in (over - 1, over, -over, 1 << 19000):
        for convert in (str, repr, lambda number: repr([number]), lambda number: str(ValueError(number))):
            try:
                print(len(convert(value)))
            except ValueError as error:
                print(error.args)
