def key_callbacks():
    events = []

    class Key:
        def __init__(self, value):
            self.value = value

        def __hash__(self):
            events.append(("hash", self.value))
            if self.value == "bad":
                raise LookupError("from hash")
            return 42

        def __eq__(self, other):
            events.append(("eq", self.value, other.value))
            if other.value == "boom":
                raise ValueError("from equality")
            return self.value == other.value

    for make in (set, frozenset):
        for keys in ([Key("bad")], [Key("a"), Key("boom")], [Key("a"), Key("a")]):
            try:
                print("size", len(make(keys)))
            except (LookupError, ValueError) as error:
                print(type(error).__name__, str(error))
    one = Key("copy")
    mutable = set([one])
    frozen = frozenset(mutable)
    print(len(set(frozen)), {frozen: 7}[frozen], events)


def current():
    key_callbacks()
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
    print("ok")


if __name__ == "__main__":
    current()


class ComparedKey:
    def __hash__(self):
        return 11

    def __eq__(self, other):
        compared.append("equal")
        if fail_comparison:
            raise LookupError("set comparison")
        return True


compared = []
fail_comparison = False
left = {ComparedKey()}
right = frozenset([ComparedKey()])
assert left == right and not right != left
assert compared == ["equal", "equal"]
fail_comparison = True
try:
    left == right
except LookupError as error:
    assert error.args == ("set comparison",)
else:
    raise AssertionError("missing element comparison")
