"""Compare dict subclass storage, inherited methods, and explicit base dispatch."""


def current():
    from collections import Counter, defaultdict
    from collections import _count_elements
    from _collections import _count_elements as native_count

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
    groups = defaultdict(list)
    saved = groups["key"]
    saved.append(7)
    print(groups.clear(), groups["key"], saved)
    print(defaultdict.clear is dict.clear, dict.clear(groups), len(groups))

    groups["visits"].append(1)
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
