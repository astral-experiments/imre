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
    class StoredHash:
        blocked = False

        def __hash__(self):
            if self.blocked:
                raise RuntimeError("rehashed stored key")
            return 7

        def __eq__(self, other):
            return isinstance(other, StoredHash)

    key = StoredHash()
    source = {key: 1}
    key.blocked = True
    assert len(set(source)) == len(frozenset(source)) == 1
    updated = set()
    updated.update(source)
    assert len(updated) == 1
    a, b = StoredHash(), StoredHash()
    left, right = {a}, {b}
    a.blocked = b.blocked = True
    assert left.issuperset(right)
    assert frozenset(left).issuperset(frozenset(right))
    print("ok")


if __name__ == "__main__":
    current()
