from collections import Counter, namedtuple


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
    from annotationlib import Format
    from typing import NamedTuple

    Typed = NamedTuple("Typed", [("x", int), ("tag", None)])
    assert Typed.__module__ == __name__
    assert Typed.__orig_bases__ == (NamedTuple,)
    assert Typed.__annotations__ is Typed.__new__.__annotations__
    assert Typed.__annotate__(Format.STRING) == {"x": "int", "tag": "NoneType"}
    assert Typed(tag=None, x=3) == (3, None)

    class Declared(NamedTuple):
        label: str

    assert Declared.__orig_bases__ == (NamedTuple,)
    assert Declared("kept").label == "kept"
    assert Declared("kept")._asdict() == {"label": "kept"}
    assert Declared.__replace__ is Declared._replace
    updated = Declared("kept")._replace(label="new")
    assert type(updated) is Declared and updated.label == "new"

    Point = namedtuple('Point', 'self y')
    class Pixel(Point):
        pass
    before = Pixel(1, 2)
    after = before._replace(self=3)
    assert type(after) is Pixel and tuple(after) == (3, 2) and tuple(before) == (1, 2)
    assert Point.__replace__ is Point._replace

    items = []
    bound = Point(items, items)._asdict
    result = bound()
    assert type(result) is dict and list(result) == ["self", "y"]
    assert result["self"] is result["y"] is items and result is not bound()

    class Reordered(Point):
        _fields = ("a", "b", "a", "extra")

        def __iter__(self):
            return iter((2, 4, 6))

    assert Point._asdict(self=Reordered(1, 2)) == {"a": 6, "b": 4}

    try:
        before._replace(y=4, width=5)
    except TypeError as error:
        assert str(error) == "Got unexpected field names: ['width']"
    else:
        raise AssertionError("unknown field name accepted")

    assert namedtuple.__module__ == "collections" and namedtuple.__name__ == "namedtuple"
    assert type(namedtuple) is type(current)
    assert ">>> Point =" in namedtuple.__doc__
    assert ">>> c = Counter(" in Counter.__doc__ and ">>> Counter(" in Counter.most_common.__doc__
    Fields = namedtuple("Fields", "left right")
    new = Fields.__new__
    names = new.__code__.co_varnames
    assert names == ("_cls", "left", "right")
    assert names is new.__code__.co_varnames and names[1] is Fields._fields[0]
    assert new.__code__.co_nlocals == len(names)
    try:
        new(list, 1, 2)
    except TypeError as error:
        trace = error.__traceback__.tb_next
        assert trace.tb_frame.f_code is new.__code__ and trace.tb_lineno == 1
    else:
        raise AssertionError("tuple allocator accepted list")

    Column = namedtuple("Column", ["K"])
    assert Column._fields == ("K",) and Column(K=4)[0] == 4
    try:
        namedtuple("Collision", ["K", "K"])
    except SyntaxError as error:
        assert error.msg == "duplicate argument 'K' in function definition"
        assert (error.lineno, error.offset, error.end_offset) == (1, 17, 20)
    else:
        raise AssertionError("duplicate constructor parameter accepted")
    try:
        namedtuple("Constant", ["K", "K", "Ｔｒｕｅ"])
    except ValueError as error:
        assert str(error) == "identifier field can't represent 'True' constant"
    else:
        raise AssertionError("constant constructor parameter accepted")

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

    lengths = []

    class Measured(Fields):
        def __len__(self):
            lengths.append(1)
            return len(lengths)

    class Inherited(Measured):
        pass

    measured = Inherited(2, 3)
    measured.__len__ = lambda: 99
    assert len(measured) == 1 and tuple.__len__(measured) == 2
    lengths.clear()
    try:
        Inherited._make((2, 3))
    except TypeError as error:
        assert str(error) == "Expected 2 arguments, got 2"
    else:
        raise AssertionError("subclass length ignored")
    assert lengths == [1, 1]

    Single = namedtuple("Single", "only")
    rebound = Fields._make.__func__(Single, (7, 8))
    assert type(rebound) is Single and tuple(rebound) == (7, 8)

    class Tagged(tuple):
        def __init__(self, values, *, tag):
            self.tag = tag

    tagged = Tagged((1, 2), tag="set by init")
    assert tagged == (1, 2) and tagged.tag == "set by init"
    assert tuple.__new__(Tagged, (3,), tag="unused") == (3,)

    from copyreg import _reconstructor
    assert tuple.__init__ is Fields.__init__ is object.__init__
    assert _reconstructor(Fields, tuple, (1, 2)) == (1, 2)
    assert Fields(1, 2).__init__(1, 2) is None
    assert tuple.__init__((), ()) is None
    try:
        tuple.__init__(tagged, ())
    except TypeError:
        pass
    else:
        raise AssertionError("extra arguments with an overridden initializer")

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
    assert not isinstance(shadow, type("tuple", (), {}))

    slot = type(getter).__repr__
    bound = getter.__repr__
    assert slot.__objclass__ is type(getter)
    assert slot.__get__(getter)() == bound() == "_tuplegetter(1, 'Alias for field number 1')"
    getter.__doc__ = "right coordinate"
    assert slot(getter) == bound() == repr(getter)
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
