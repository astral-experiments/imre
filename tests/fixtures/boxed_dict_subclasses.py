"""Compare dict subclass storage, inherited methods, and explicit base dispatch."""


def ordered_dictionary():
    from collections import OrderedDict

    source = OrderedDict(b=[2], a=[1])
    assert source != OrderedDict(a=[1], b=[2])
    assert source == {"a": [1], "b": [2]}
    dense = OrderedDict([(2, "two"), (0, "zero"), (1, "one")])
    assert list(reversed(dense)) == [1, 0, 2]
    items = reversed(dense.items())
    assert next(items) == (1, "one")
    dense[0] = "updated"
    assert list(items) == [(0, "updated"), (2, "two")]

    class NoKeys(OrderedDict):
        def keys(self):
            raise AssertionError("comparison called keys")

    assert NoKeys(b=2) == {"b": 2}
    shallow = source.copy()
    assert type(shallow) is OrderedDict and shallow["b"] is source["b"]
    cursor = iter(source.items())
    assert next(cursor) == ("b", [2])
    del source["a"]
    source["a"] = [3]
    try:
        next(cursor)
    except RuntimeError as error:
        assert str(error) == "OrderedDict mutated during iteration"
    else:
        raise AssertionError("cursor missed a deletion and reinsertion")
    assert next(cursor, None) is None
    endpoints = OrderedDict(a=1, b=2, c=3)
    assert endpoints.popitem(last=False) == ("a", 1)
    assert endpoints.popitem() == ("c", 3)
    assert list(endpoints.items()) == [("b", 2)]
    source.clear()
    source["self"] = source
    assert repr(source) == "OrderedDict({'self': ...})"

    moved = OrderedDict(a=1, b=2, c=3)
    moved.move_to_end("a")
    assert list(moved) == ["b", "c", "a"]
    assert list(dict.keys(moved)) == ["a", "b", "c"]
    assert moved.popitem(last=False) == ("b", 2)


    class ClearOnRepr:
        def __repr__(self):
            snapshot.clear()
            return "first"

    snapshot = OrderedDict([("a", ClearOnRepr()), ("b", 2)])
    assert repr(snapshot) == "OrderedDict({'a': first, 'b': 2})"
    assert not snapshot


def inherited_deletion():
    from operator import delitem

    class Data(dict):
        pass

    class Child(Data):
        pass

    class Override(Data):
        def __delitem__(self, key):
            self.last_key = key
            return "ignored"

    data = Child(a=1, b=2, c=3)
    print(Child.__delitem__ is dict.__delitem__)
    print(delitem(data, "a"), dict(data))
    del data["b"]
    print(dict.__delitem__(data, "c"), dict(data))
    override = Override(x=4)
    print(delitem(override, "x"), override.last_key, dict(override))
    print(dict.__delitem__(override, "x"), dict(override))


def representation_descriptor():
    class Inherited(dict):
        pass

    class Override(Inherited):
        def __repr__(self):
            return "custom mapping"

    from collections import defaultdict

    slot = dict.__repr__
    print(type(slot).__name__, slot.__name__, slot.__qualname__, slot.__objclass__ is dict)
    print({slot: "dictionary"}[Inherited.__repr__], slot.__get__(None, Inherited) is slot)
    data = Override(a=1)
    bound = slot.__get__(data, Override)
    print(type(bound).__name__, bound.__self__ is data, bound(), repr(data))
    data["a"] = 2
    print(slot(data), Inherited(a=3).__repr__())
    default = defaultdict(list)
    default["value"] = 12
    print(slot(default))
    recursive = {}
    recursive["again"] = recursive
    print(slot(recursive))
    for call in (lambda: slot(), lambda: slot(1), lambda: slot({}, 2),
                 lambda: slot({}, key=2), lambda: slot.__get__(1, dict)):
        try:
            call()
        except TypeError as exc:
            print(str(exc))


def mapping_patterns():
    from collections import defaultdict

    matched = defaultdict(list)
    matched["keep"] = None
    matched["other"] = []
    match matched:
        case {"missing": _}:
            raise AssertionError("looked up a missing key through __missing__")
        case {"keep": None, **rest}:
            assert type(rest) is dict and list(rest) == ["other"]
            assert rest["other"] is matched["other"] and "missing" not in matched
        case _:
            raise AssertionError("present None did not match")

    calls = []

    class Keys:
        @property
        def key(self):
            calls.append("key")
            return "a"

    keys = Keys()
    for subject in ({"a": 1}, {"a": 1, "b": 2}):
        try:
            match subject:
                case {keys.key: _, keys.key: _}:
                    raise AssertionError("matched duplicate keys")
        except ValueError:
            assert calls == ["key", "key"]
        else:
            assert not calls


def current():
    from collections import defaultdict

    class Display(defaultdict):
        pass
    display = Display(list, a=2)
    assert repr(display) == display.__repr__() == defaultdict.__repr__(display)
    assert repr(display) == "Display(<class 'list'>, {'a': 2})"
    assert dict.__repr__(display) == "{'a': 2}"
    slot = defaultdict.__repr__
    assert slot.__objclass__ is defaultdict
    assert slot.__get__(display, Display)() == repr(display)
    assert type(slot) is type(dict.__repr__)
    assert type(display.__repr__) is type({}.__repr__)
    deep = None
    for _ in range(160):
        deep = Display(None, child=deep)
    assert len(repr(deep)) == len("None") + 160 * len("Display(None, {'child': })")

    entries = defaultdict(list, [("saved", 2)], extra=3, saved=5)
    assert entries == {"saved": 5, "extra": 3}
    assert entries["missing"] == []
    assert defaultdict(default_factory=1) == {"default_factory": 1}

    from collections import OrderedDict

    class Key:
        def __hash__(self):
            if self.fail:
                raise LookupError("iterator hash")
            return 17

    key = Key()
    key.fail = False
    cursor = iter(OrderedDict([(key, 10)]))
    key.fail = True
    try:
        next(cursor)
    except LookupError as error:
        assert str(error) == "iterator hash"
    else:
        raise AssertionError("missing hash exception")
    assert next(cursor, None) is None

    from collections import Counter, defaultdict
    from collections import _count_elements
    from _collections import _count_elements as native_count
    from copy import copy

    fromkeys_construction()
    ordered_dictionary()
    mapping_patterns()
    counts = Counter(a=3)
    clone = copy(counts)
    clone["a"] += 1
    assert type(clone) is Counter and clone["missing"] == 0 and counts["a"] == 3
    groups = defaultdict(list)
    class SlottedFactory(defaultdict):
        __slots__ = ("default_factory",)

    slotted = SlottedFactory(list)
    assert not hasattr(slotted, "default_factory")
    slotted.default_factory = tuple
    assert slotted.default_factory is tuple and slotted["first"] == []
    del slotted.default_factory
    assert not hasattr(slotted, "default_factory") and slotted["second"] == []

    child = groups["present"]
    clone = copy(groups)
    assert type(clone) is defaultdict and clone.default_factory is list
    assert clone["present"] is child and clone["missing"] == [] and "missing" not in groups
    left, right = Counter(b=2, a=-3), Counter(a=5, c=4)
    assert list((left + right).items()) == [("b", 2), ("a", 2), ("c", 4)]
    assert -left == Counter(a=3) and +left == Counter(b=2)
    alias = left
    left |= right
    assert alias is left and list(alias.items()) == [("b", 2), ("a", 5), ("c", 4)]
    left -= left
    assert not alias
    counted = {"a": 4}
    print(native_count is _count_elements, native_count(counted, "aba"), counted)
    try:
        native_count(None, 3)
    except TypeError as error:
        print(type(error).__name__)

    counts = Counter(a=10, b=5, c=0)
    print(counts.total(), counts["absent"], len(counts))
    counts.update("ba", a=1 << 100)
    counts.subtract({"a": 1 << 100, "b": 9}, c=1)
    copied = counts.copy()
    print(repr(counts), str(Counter()), [counts])
    print(counts.most_common(1), Counter.most_common(counts, n=0))
    print(Counter("bacab").most_common(1 << 100))
    del counts["c"]
    del counts["absent"]
    print(list(counts.items()), copied.total(), copied["absent"])
    plain = Counter(a=1)
    with_zero = Counter(a=1, b=0)
    print(plain == with_zero, plain != with_zero, with_zero == dict(plain))
    print(Counter(b=-1) < Counter(), plain <= with_zero, with_zero > plain)
    counts = Counter(a=2, b=1, ignored=-3)
    elements = counts.elements()
    print(next(elements))
    counts["a"] = 0
    counts["b"] = 2
    print(list(elements))

    order = counts.keys()
    print(Counter.clear(counts), list(order), counts["absent"])
    class Factory:
        def __call__(self):
            return []

    factory = Factory()
    assert defaultdict(factory).default_factory is factory
    assert defaultdict(None).default_factory is None
    try:
        defaultdict(37)
    except TypeError as error:
        assert error.args == ("first argument must be callable or None",)
    else:
        raise AssertionError("a non-callable defaultdict factory was accepted")

    groups = defaultdict(list)
    saved = groups["key"]
    saved.append(7)
    print(groups.clear(), groups["key"], saved)
    print(defaultdict.clear is dict.clear, dict.clear(groups), len(groups))

    groups["visits"].append(1)
    class WithFactory(defaultdict):
        pass

    derived = WithFactory(list)
    assert derived.default_factory is list
    derived["first"].append(2)
    assert derived["first"] == [2]
    assert WithFactory().default_factory is None

    plain_copy = dict.copy(groups)
    other = groups.copy()
    other["visits"].append(2)
    print(plain_copy["visits"], groups["visits"], other["fresh"], list(groups))
    print(type(plain_copy) is dict, type(other) is defaultdict, isinstance(other, dict), "fresh" in plain_copy)

    class ClearOnCompare:
        def __hash__(self):
            return 1

        def __eq__(self, other):
            collision.clear()
            return False

    collision = {}
    for _ in range(3):
        collision[ClearOnCompare()] = 0
    print(len(collision))

    inherited_deletion()
    representation_descriptor()
    class Data(dict):
        pass

    empty = Data()
    print(isinstance(empty, dict), bool(empty), empty.get("visits", 0), len(empty))
    source = {"visits": 1}
    data = Data(source, greeting="hello")
    data.visits = "attribute"
    keys = data.keys()
    data["visits"] += 1
    data["more"] = "key"
    print(data.visits, dict.get(data, "visits"), list(keys), len(data))
    print(dict(data), {**data}, data == dict(data), source)
    print("visits" in data, "absent" in data, data.get("absent"))
    print(Data.get is dict.get, Data.__setitem__ is dict.__setitem__)

    class Tracked(Data):
        def __init__(self, values):
            self.writes = []
            dict.__init__(self, values)

        def __setitem__(self, key, value):
            self.writes.append(key)
            dict.__setitem__(self, key, value)

        def __missing__(self, key):
            return "missing " + key

        def __bool__(self):
            return False

    session = Tracked({"visits": 1})
    values = session.values()
    print(bool(session), bool(values), session.writes, session["absent"], session.get("absent"))
    session["visits"] = session.get("visits", 0) + 1
    print(session.writes, list(values), session["visits"])
    print(dict.__setitem__(session, "base", 9), dict.__init__(session, {"another": 10}))
    dict.update(session, [("key", 11)])
    print(session.writes, session)
    data.get = lambda key: "shadow"
    print(data.get("visits"), dict.get(data, "visits"))

    pairs = dict.__iter__({"first": None, "second": None, "third": None})
    from operator import length_hint

    print(length_hint(pairs))
    for key in pairs:
        print(key)
        break
    print(length_hint(pairs))

    class Shared(dict):
        def __iter__(self):
            return pairs

    for key in Shared():
        print(key)
        break
    print(length_hint(pairs), list(pairs), length_hint(pairs))

    live = {"first": 1, "second": 2}
    live_items = iter(live.items())
    live_values = iter(live.values())
    print(next(live_items), next(live_values))
    live["second"] = 99
    print(next(live_items), next(live_values))
    live["third"] = 3
    for cursor in (live_items, live_values):
        try:
            next(cursor)
        except RuntimeError as error:
            print(error, length_hint(cursor))

    def replace_and_append(value):
        if value == 1:
            sequence[1] = 3
            sequence.append(4)
        return value

    sequence = [1, 2]
    visited = []
    for value in sequence:
        visited.append(replace_and_append(value))
    print(visited)
    sequence = [1, 2]
    print([replace_and_append(value) for value in sequence])

    class Filtered(Data):
        def __iter__(self):
            raise AssertionError("dict copying uses keys")

        def keys(self):
            return ("visits",)

        def __getitem__(self, key):
            return dict.__getitem__(self, key) + 10

    filtered = Filtered(data)
    print(dict(filtered), {**filtered}, filtered.copy())

    class MissingKeys(Filtered):
        @property
        def keys(self):
            raise AttributeError("unavailable keys")

    try:
        dict.copy(MissingKeys(data))
    except AttributeError as error:
        print(error.args)

    pending = []

    class FirstPair(dict):
        def __iter__(self):
            pending[1] = ("replaced", 3)
            pending.append(("appended", 2))
            return dict.__iter__({"first": None, 1: None})

    pending.extend([FirstPair(), ("original", 4)])
    print(dict(pending))

    class BrokenPair(dict):
        def __iter__(self):
            raise TypeError("from pair iterator")

    for pair in (42, BrokenPair()):
        partial = {"kept": 1}
        try:
            partial.update([("good", 4), pair, ("late", 9)], tail=1)
        except TypeError as error:
            print(error.args, partial)

    pairs = dict.__iter__({("first", 1): None, ("bad",): None, ("later", 2): None})
    copied = {}
    try:
        copied.update(pairs)
    except ValueError:
        print(copied, list(pairs))

    class Probed:
        def __init__(self, fail=False):
            self.lookups = 0
            self.fail = fail

        @property
        def keys(self):
            self.lookups += 1
            if self.lookups == 1:
                return lambda: ["first"]
            if self.fail:
                raise ValueError("second lookup")
            return lambda: ["second"]

        def __getitem__(self, key):
            return key

    probed = Probed()
    print(dict(probed), probed.lookups)
    probed = Probed()
    print({**probed}, probed.lookups)
    try:
        received = {"kept": 1}
        received.update(Probed(True))
    except ValueError as error:
        print(error.args, received)

    class FailsLater(Filtered):
        def keys(self):
            return ("visits", "absent")

    received = {}
    try:
        received.update(FailsLater(data))
    except KeyError as error:
        print(received, error.args)

    class MutableKeys:
        def __init__(self, as_view):
            self.names = [("first", 1), ("before", 2)]
            self.by_index = {0: self.names[0], 1: self.names[1]}
            self.as_view = as_view

        def keys(self):
            return self.by_index.values() if self.as_view else self.names

        def __getitem__(self, key):
            if key[0] == "first":
                self.names[1] = ("after", 3)
                self.by_index[1] = self.names[1]
                for _ in range(6000):
                    []
            return key[1]

    print(dict(MutableKeys(False)), dict(MutableKeys(True)))

    def only_view():
        owner = Data(held=["live"])
        return owner.items()

    view = only_view()
    for _ in range(6000):
        []
    print(list(view), list(values), session.writes)

    class ViewPair(dict):
        def __iter__(self):
            source["b"] = ("new", 2)
            if grow:
                source["c"] = ("later", 3)
            return dict.__iter__({"first": None, 1: None})

    for grow in (False, True):
        source = {"a": ViewPair(), "b": ("old", 0)}
        copied = {}
        try:
            copied.update(source.values())
        except RuntimeError as error:
            print(error.args)
        print(copied)

    from operator import delitem

    source = {"first": 1}
    cursor = iter(source)
    key = next(cursor)
    delitem(source, key)
    source[key] = 2
    try:
        next(cursor)
    except RuntimeError as error:
        print(error, length_hint(cursor))

    source = {"a": 1, "b": 2, "c": 3}
    cursor = dict.__iter__(source)
    for first in cursor:
        break
    for key in ("a", "b", "c"):
        delitem(source, key)
    for key in ("d", "e", "f"):
        source[key] = key
    print(first, list(cursor))

    try:
        empty[()]
    except KeyError as error:
        print(error.args)
    for method, args in ((dict.get, ()), (dict.get, (1, "x")), (dict.__setitem__, (1, "x", 2))):
        try:
            method(*args)
        except TypeError as error:
            print(error.args)

    class RefusingHash:
        def __hash__(self):
            raise RuntimeError("hash interrupted")

    class Colliding:
        def __hash__(self):
            return 17

        def __eq__(self, other):
            raise ValueError("comparison interrupted")

    class EqualOnly:
        def __eq__(self, other):
            return True

    for key in (RefusingHash(), Colliding(), EqualOnly()):
        target = {Colliding(): "old"}
        try:
            target.update([("first", 1), (key, "new"), ("late", 2)])
        except Exception as error:
            print(type(error).__name__, list(target.values()))

    class Reentrant:
        def __hash__(self):
            return 9

        def __eq__(self, other):
            delitem(target, self)
            for _ in range(6000):
                []
            return True

    target = {Reentrant(): "old"}
    target[Reentrant()] = "new"
    print(list(target.values()))

    class Once:
        def __init__(self):
            self.calls = 0

        def __hash__(self):
            self.calls += 1
            if self.calls > 1:
                raise RuntimeError("rehashed a stored key")
            return 5

    once = Once()
    source = {once: "value"}
    print(list(dict(source).values()), once.calls)

    from collections import defaultdict

    groups = defaultdict(list)
    existing = groups["key"]
    assert repr(groups) == "defaultdict(<class 'list'>, {'key': []})"
    groups["cycle"] = groups
    assert repr(groups).endswith("'cycle': defaultdict(<class 'list'>, {...})})")
    del groups["cycle"]
    print(groups.default_factory is list, groups.__missing__("key") is existing)
    groups.default_factory = None
    try:
        defaultdict.__missing__(groups, "key")
    except KeyError as error:
        print(error.args, groups["key"] is existing)

    def fill():
        groups["new"] = [7]
        return []

    groups.default_factory = fill
    print(groups["new"], list(groups))

    class Late:
        pass

    retained = Late()
    target = {retained: 7}
    Late.__eq__ = lambda left, right: left is right
    target[Late()] = 11
    print(target[retained], len(target))

    class Child(EqualOnly):
        pass

    del EqualOnly.__eq__
    print(EqualOnly.__hash__ is None)
    for key in (EqualOnly(), Child()):
        try:
            target[key] = 41
        except TypeError:
            print("implicit hash disabled")

    del Colliding.__hash__
    key = Colliding()
    print({key: 13}[key])
    declined_key_equality()
    missing_key_errors()


def fromkeys_construction():
    from collections import Counter, defaultdict

    class Custom(dict):
        def __setitem__(self, key, value):
            dict.__setitem__(self, key + 10, value)

    source = Custom()
    shared = []
    made = source.fromkeys([2, 1, 2], shared)
    made[11].append(7)
    assert type(made) is Custom and list(made) == [12, 11]
    assert not source and made[12] is shared and shared == [7]
    bound = defaultdict.fromkeys
    made = bound([2, 1, 2], shared)
    assert type(made) is defaultdict and made.default_factory is None
    assert list(made) == [2, 1] and made[1] is made[2] is shared
    fresh = defaultdict(list).fromkeys([1])
    assert fresh[1] is None
    try:
        fresh[2]
    except KeyError:
        pass
    else:
        raise AssertionError("inherited the original factory")
    try:
        Counter.fromkeys([1])
    except NotImplementedError:
        pass
    else:
        raise AssertionError("Counter.fromkeys accepted keys")


def declined_key_equality():
    calls = []

    class Key:
        def __init__(self, name):
            self.name = name

        def __hash__(self):
            return 7

        def __eq__(self, other):
            calls.append((self.name, other.name))
            return NotImplemented

    left, right = Key("left"), Key("right")
    for container in ({left: 1}, {left}):
        assert right not in container
        assert calls == [("left", "right"), ("right", "left")]
        calls.clear()


def missing_key_errors():
    calls = []

    class Defaults(dict):
        def __missing__(self, key):
            calls.append(key)
            return 12

    class BadHash:
        def __hash__(self):
            raise KeyError("hash failed")

    table = Defaults()
    try:
        table[BadHash()]
    except KeyError as error:
        assert error.args == ("hash failed",)
    else:
        raise AssertionError("lookup suppressed a hashing exception")
    assert table["absent"] == 12
    assert calls == ["absent"]
