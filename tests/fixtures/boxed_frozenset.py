def current():
    value = frozenset("aβaaa")
    assert type(value) is frozenset and isinstance(value, frozenset)
    assert not isinstance(value, set)
    assert len(value) == 2 and value and not frozenset()
    assert frozenset(value) is value and value.copy() is value
    assert value == {"β", "a"} and value == frozenset(["β", "a"])
    assert value != ("a", "β") and value != {"a"}
    assert value.issuperset("βa") and not value.issuperset("βb")
    assert value.issuperset([]) and set("aβ").issuperset(value)
    assert repr(frozenset()) == "frozenset()" and repr(frozenset([2])) == "frozenset({2})"

    original = {1, 2}
    frozen = frozenset(original)
    original.add(3)
    assert 3 not in frozen and frozenset([2, True]) == frozen
    assert {frozen: "found"}[frozenset([2, 1])] == "found"
    assert {2, 1} in frozenset([frozen]) and frozen in frozenset([frozen])
    assert not hasattr(frozen, "add") and not hasattr(frozen, "update")

    remaining = {0: None, 1: None}.__iter__()
    assert not frozen.issuperset(remaining) and tuple(remaining) == (1,)
    assert not frozen.issuperset((0, []))
    for invalid in ([], ([],), bytearray(1), {1}):
        try:
            frozenset([invalid])
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("unhashable element accepted")
        try:
            frozen.issuperset([invalid])
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("unhashable argument accepted")
    from types import GenericAlias
    alias = GenericAlias(list, (0,))
    equal_alias = GenericAlias(list, (0,))
    assert alias == equal_alias and equal_alias in {alias}
    assert {alias: "value"}[equal_alias] == "value"
    left, right = 0, 0
    for _ in range(30000):
        left = frozenset((left,))
        right = frozenset((right,))
    assert left == right
    left, right = 0, 0
    for _ in range(30000):
        left = frozenset((left, "sentinel"))
        right = frozenset((right, "sentinel"))
    assert left == right
    left, right = 0, 0
    for _ in range(10000):
        left = frozenset(((left,),))
        right = frozenset(((right,),))
    assert left == right
    left, right = 0, 0
    for _ in range(10000):
        left = frozenset((GenericAlias(list, (left,)),))
        right = frozenset((GenericAlias(list, (right,)),))
    assert left == right
    original = {1, 2}
    copied = original.copy()
    copied.add(3)
    assert original == {1, 2} and copied == {1, 2, 3} and copied is not original
    values = {1, 2}
    assert values.discard(1) is None and values == {2}
    assert values.discard(3) is None and values == {2}
    left = {1, 2}
    right = frozenset((2, 3))
    assert left - right == {1} and right & left == frozenset((2,))
    assert type(right - left) is frozenset
    alias = left
    left &= right
    assert left is alias and alias == {2}
    mapping = {2: None}
    keys = mapping.keys()
    mapping[3] = None
    assert keys & [1, 3] == {3} and [1, 3] - keys == {1}
    left -= keys
    assert left == set() and alias == {2}
    for kind, empty, one in ((set, "set()", "{1}"), (frozenset, "frozenset()", "frozenset({1})")):
        slot = kind.__repr__
        value = kind([1])
        assert type(slot).__name__ == "wrapper_descriptor"
        assert slot.__objclass__ is kind and slot is kind.__repr__
        assert slot(kind()) == empty and slot(value) == one
        assert slot.__get__(value)() == one and value.__repr__() == one

    class Mutable(set):
        def __repr__(self):
            return "override"

    class Frozen(frozenset):
        pass

    assert set.__repr__(Mutable([1])) == "Mutable({1})"
    assert repr(Mutable([1])) == "override"
    assert frozenset.__repr__(Frozen([1])) == "Frozen({1})"
    assert Frozen.__repr__ is frozenset.__repr__

    from weakref import proxy

    for owner, target in ((set, Mutable()), (frozenset, Frozen())):
        receiver = proxy(target)
        assert isinstance(receiver, owner)
        name = owner.__name__
        for operation, message in (
            (lambda: owner.__repr__(receiver),
             f"descriptor '__repr__' requires a '{name}' object but received a 'weakref.ProxyType'"),
            (lambda: owner.__repr__.__get__(receiver, owner),
             f"descriptor '__repr__' for '{name}' objects doesn't apply to a 'weakref.ProxyType' object"),
        ):
            try:
                operation()
            except TypeError as error:
                assert str(error) == message
            else:
                raise AssertionError("set repr descriptor accepted a weak proxy")

    key = 0
    for _ in range(65536):
        key = GenericAlias(list, key)
    assert key in {key}
    print("ok")


if __name__ == "__main__":
    current()
