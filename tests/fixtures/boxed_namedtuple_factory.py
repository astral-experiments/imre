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


def layout_read_allocation():
    import os

    # Match the allocation size recognized by the boxed-call malloc wrapper.
    field = "f" * (113 * 8)
    row = namedtuple("Row", (field, "kept"))(7, 9)
    marker = {}
    os.putenv("IMRE_TEST_GETTER_ALLOCATION", "1")
    marker[0] = 1
    text = repr(row)
    os.unsetenv("IMRE_TEST_GETTER_ALLOCATION")
    assert text == f"Row({field}=7, kept=9)"
    print(row.kept)


def current():
    Fields = namedtuple("Fields", "left right")
    getter = Fields.right
    assert getter.__get__((2, 3), tuple) == 3
    assert getter.__get__(Fields(4, 5)) == 5

    class Shift(Fields):
        def __new__(cls, left, right):
            return super().__new__(cls, left + 1, right + 2)

    class Foreign(Fields):
        def __new__(cls, left):
            return left

        def __init__(self, left):
            raise AssertionError("foreign result was initialized")

    assert tuple(Shift(2, 3)) == (3, 5)
    assert tuple(Shift._make((2, 3))) == (2, 3)
    assert Foreign(9) == 9
    short = tuple.__new__(Fields, (7,))
    assert {short: "one"}[(7,)] == "one" and short.left == 7
    try:
        short.right
    except IndexError:
        pass
    else:
        raise AssertionError("missing tuple position")

    class Shadow(Fields):
        left = "shadow"

    shadow = Shadow(3, 4)
    assert shadow.left == "shadow"
    shadow.left = 8
    assert shadow.left == 8 and Fields.left.__get__(shadow) == 3

    getter.__doc__ = "right coordinate"
    assert repr(getter) == "_tuplegetter(1, 'right coordinate')"
    assert type(getter).__doc__.__get__(getter) == "right coordinate"

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

    class Positional(tuple):
        pass

    class Remapped(Match):
        def __iter__(self):
            return iter((31, 47, 59))

    for pair in (Positional((1, 2, 3)), Remapped(1, 2, 3)):
        print("%s/%s/%s" % pair, b"%o/%o/%o" % pair, tuple(iter(pair)))
    print(repr(Remapped(1, 2, 3)))
