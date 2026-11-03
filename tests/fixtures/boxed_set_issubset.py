def tail():
    yield 1
    yield 1
    yield 2
    raise RuntimeError("tail reached")


class MutatingIterable:
    def __init__(self, receiver):
        self.receiver = receiver

    def __iter__(self):
        try:
            yield 1
            raise AssertionError("subset consumed the iterator tail")
        finally:
            self.receiver.add(2)


class SetSubclass(set):
    def __iter__(self):
        raise AssertionError("set storage must not call __iter__")

    def __contains__(self, item):
        raise AssertionError("set storage must not call __contains__")

    def __len__(self):
        raise AssertionError("set storage must not call __len__")


class Overridden(frozenset):
    def issubset(self, other):
        return "override"


class Key:
    def __init__(self, name, events):
        self.name = name
        self.events = events

    def __hash__(self):
        self.events.append(("hash", self.name))
        return 11

    def __eq__(self, other):
        self.events.append(("eq", self.name, other.name))
        return True


class ReplacedKey:
    def __init__(self, receiver):
        self.receiver = receiver

    def __hash__(self):
        return 1

    def __del__(self):
        self.receiver.add(6)


class RemovingKey:
    def __init__(self, receiver):
        self.receiver = receiver

    def __hash__(self):
        return 1

    def __eq__(self, key):
        self.receiver.discard(key)
        return True


class BrokenHash:
    def __hash__(self):
        raise ValueError("hash reached")


class HashError(TypeError):
    pass


class RaisesHash:
    def __init__(self, error):
        self.error = error

    def __hash__(self):
        raise self.error


class BrokenEquality:
    def __hash__(self):
        return 11

    def __eq__(self, other):
        raise ValueError("equality reached")


def check_disjoint():
    class Mutable(set):
        pass

    class Frozen(frozenset):
        pass

    assert Mutable.isdisjoint is set.isdisjoint
    assert Frozen.isdisjoint is frozenset.isdisjoint
    assert type(Mutable.isdisjoint).__name__ == "method_descriptor"

    class Key:
        locked = False
        def __hash__(self):
            assert not self.locked
            return 53

    key = Key()
    left, right = {key}, frozenset((key,))
    key.locked = True
    assert not set.isdisjoint(left, right)
    assert not frozenset.isdisjoint(right, left)

    class Child(set):
        def __iter__(self):
            yield 2
            raise AssertionError("advanced past the common key")

    other = Child([9])
    assert not other.isdisjoint(other)
    assert not {2}.isdisjoint(other)
    assert not {2}.isdisjoint(iter([1, 2, []]))
    assert frozenset((5,)).isdisjoint([6, 6])
    try:
        set().isdisjoint([set()])
    except TypeError:
        pass
    else:
        raise AssertionError("accepted an unhashable key")


def current():
    check_disjoint()
    for kind in (set, frozenset):
        members = kind([1, 2])
        method = members.issubset
        assert method(members)
        assert method([2, 1]) and not method([1, 1])
        assert method({1: None, 2: None}) and not method({2})
        assert kind.issubset(members, SetSubclass([1, 2, 3]))
        assert kind.issubset.__get__(members)([1, 2])
        assert type(kind.issubset).__name__ == "method_descriptor"
        source = tail()
        assert method(source)
        try:
            next(source)
        except RuntimeError as exc:
            assert str(exc) == "tail reached"
        else:
            raise AssertionError("remaining iterator exception was lost")
        try:
            kind().issubset(tail())
        except RuntimeError as exc:
            assert str(exc) == "tail reached"
        else:
            raise AssertionError("empty receiver failed to consume iterable")
        for members in (kind(), kind([2])):
            for bad in ([], {}, set()):
                try:
                    members.issubset([1, bad])
                except TypeError as exc:
                    assert exc.args == ("unhashable type: '" + type(bad).__name__ + "'",)
                else:
                    raise AssertionError("unhashable iterable element accepted")
            for error in (TypeError("custom", 5), HashError("subclass")):
                try:
                    members.issubset([RaisesHash(error)])
                except TypeError as exc:
                    assert exc is error
                else:
                    raise AssertionError("custom hash exception lost")
        try:
            kind().issubset(None)
        except TypeError:
            pass
        else:
            raise AssertionError("invalid subset operand accepted")
        for call in (lambda: method(), lambda: method(other=[]), lambda: kind.issubset([], [])):
            try:
                call()
            except TypeError as exc:
                print(str(exc))
            else:
                raise AssertionError("invalid descriptor call accepted")

    receiver = {1}
    assert not receiver.issubset(MutatingIterable(receiver))
    assert receiver == {1, 2}

    receiver = {1}
    retained = iter(MutatingIterable(receiver))
    assert receiver.issubset(retained)
    assert receiver == {1}
    retained.close()
    assert receiver == {1, 2}

    for compare in (set.issubset, lambda left, right: left <= right):
        receiver = set()
        receiver.add(ReplacedKey(receiver))
        assert not compare(receiver, {RemovingKey(receiver)})
        assert receiver == {6}

    assert SetSubclass([1]).issubset({1})
    assert SetSubclass.issubset(SetSubclass([1]), [1])
    overridden = Overridden([1])
    assert overridden.issubset([]) == "override"
    assert frozenset.issubset(overridden, [1])

    events = []
    left_key = Key("left", events)
    right_key = Key("right", events)
    left, right = {left_key}, {right_key}
    events.clear()
    assert left.issubset(right)
    assert events == [("eq", "right", "left")], events
    events.clear()
    assert left.issubset([right_key])
    assert events == [("hash", "right"), ("eq", "left", "right")], events
    for receiver, operand, expected in (
        (set(), [BrokenHash()], "hash reached"),
        ({BrokenEquality()}, [right_key], "equality reached"),
        (left, {BrokenEquality()}, "equality reached"),
    ):
        try:
            receiver.issubset(operand)
        except ValueError as exc:
            assert str(exc) == expected
        else:
            raise AssertionError("subset swallowed a key exception")
    print("subset", events)
