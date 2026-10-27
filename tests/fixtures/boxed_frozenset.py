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
    class ComparedKey:
        def __init__(self, fail=False):
            self.fail = fail

        def __hash__(self):
            return 11

        def __eq__(self, other):
            compared.append("equal")
            if self.fail:
                raise LookupError("set comparison")
            return True

    compared = []
    left = {ComparedKey()}
    right = frozenset([ComparedKey()])
    assert left == right and not right != left
    assert compared == ["equal", "equal"]
    try:
        {ComparedKey()} == frozenset([ComparedKey(True)])
    except LookupError as error:
        assert error.args == ("set comparison",)
    else:
        raise AssertionError("missing element comparison")
    print("ok")


if __name__ == "__main__":
    current()
