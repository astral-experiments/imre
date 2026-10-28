from collections import namedtuple


class BrokenBlock:
    def __init__(self):
        self.count = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.count += 1
        if self.count <= 3:
            return self.count
        raise LookupError("error after the last field")


def current():
    Match = namedtuple("Match", "a b size")
    blocks = [(0, 0, 5), (6, 5, 0)]
    longest = Match(0, 0, 5)
    matches = list(map(Match._make, blocks))
    print(longest, matches, longest == matches[0], matches == blocks)
    print(Match.__bases__ == (tuple,), type(longest) is Match, isinstance(longest, tuple))
    print(longest.a, longest.b, longest.size, longest[-1], longest[:2], tuple(longest))
    print(Match._fields, Match.__match_args__, Match.__getitem__ is tuple.__getitem__)
    slot = Match.__getitem__
    bound = slot.__get__(longest)
    print(type(slot).__name__, slot.__objclass__ is tuple, slot.__get__(None, Match) is slot)
    print(bound.__self__ is longest, bound(-1), slot(longest, slice(1, 3)))
    try:
        slot([], 0)
    except TypeError as error:
        print(str(error))
    print(Match._make(map(int, ("0", "0", "5"))) == longest, hash(longest) == hash(blocks[0]))

    class Child(Match):
        __slots__ = ()

    other = namedtuple("Match", ("a", "b", "size"))
    print(type(Child._make(iter(blocks[1]))) is Child, isinstance(longest, other))
    print(type(Match.__repr__).__name__, Match.__repr__.__qualname__, Match.__repr__ is other.__repr__)

    class Label(Child):
        def __repr__(self):
            return "label"

    label = Label(1, 2, 3)
    print(repr(label), Match.__repr__(label), tuple.__repr__(label))
    print([Match(label, 2, 3)])
    for values in ((), (1, 2, 3, 4)):
        try:
            Match._make(iter(values))
        except TypeError as error:
            print(str(error))

    broken = BrokenBlock()
    try:
        Match._make(broken)
    except LookupError as error:
        print(broken.count, str(error))

    sentinel = []
    defaults = namedtuple("Row", "name value", defaults=[sentinel])
    print(defaults("hello").value is sentinel, defaults._field_defaults["value"] is sentinel)

    events = []

    class Field:
        def __str__(self):
            events.append("converted")
            return "first"

    def fields():
        yield Field()
        if not events:
            raise RuntimeError("field was not converted")
        yield "second"

    print(namedtuple("Ordered", fields())._fields, events)

    class BadField:
        def __str__(self):
            raise ValueError("conversion failed")

    def broken_fields():
        yield BadField()
        raise RuntimeError("iterator advanced")

    try:
        namedtuple("Broken", broken_fields())
    except ValueError as error:
        print(str(error))

    calls = []

    class Initialized(Match):
        def __init__(self, a, b, size):
            calls.append((self.a, a, b, size))

    value = Initialized(1, 2, size=3)
    print(value.a, calls)
    value = Initialized._make((4, 5, 6))
    print(type(value) is Initialized, calls)
