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
    print("ok")


if __name__ == "__main__":
    current()
