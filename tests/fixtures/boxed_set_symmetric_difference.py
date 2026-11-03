def current():
    class ColdSet(set):
        pass

    class ColdFrozen(frozenset):
        pass

    for subtype, base in ((ColdSet, set), (ColdFrozen, frozenset)):
        descriptor = subtype.symmetric_difference
        result = descriptor(subtype([1]), [2, 2])
        assert type(result) is base and result == {1, 2}

    class Key:
        def __init__(self, name):
            self.name = name
            self.locked = False

        def __hash__(self):
            if self.locked:
                raise AssertionError("rehashed a stored key")
            return 42

        def __eq__(self, other):
            comparisons.append((self.name, other.name))
            return False

    comparisons = []
    left = Key("left")
    right = Key("right")
    source = {right: 0}
    receiver = {left}
    left.locked = right.locked = True
    result = receiver.symmetric_difference(source)
    assert len(result) == 2 and len(receiver) == len(source) == 1
    assert comparisons and all(pair == ("right", "left") for pair in comparisons)

    class Child(set):
        def __iter__(self):
            raise AssertionError("iterated native set storage")

        def symmetric_difference(self, other):
            return "override"

    class FrozenChild(frozenset):
        def __iter__(self):
            raise AssertionError("iterated native frozenset storage")

    child = Child([2, 3])
    frozen = FrozenChild([1, 2])
    assert child.symmetric_difference(frozen) == "override"
    mutable_result = set.symmetric_difference(child, frozen)
    frozen_result = frozen.symmetric_difference(child)
    assert type(mutable_result) is set and mutable_result == {1, 3}
    assert type(frozen_result) is frozenset and frozen_result == {1, 3}
    assert frozenset.symmetric_difference(frozen, [2, 3, 3]) == {1, 3}

    receiver = {1}

    def source_items():
        yield 2
        receiver.add(3)
        yield 2

    bound = receiver.symmetric_difference
    assert bound(source_items()) == {1, 2, 3} and receiver == {1, 3}
    assert bound(receiver) == set()
    for call in (lambda: bound(), lambda: bound(0), lambda: bound([[]]),
                 lambda: bound([], []), lambda: bound(other=[]),
                 lambda: set.symmetric_difference(frozenset(), set()),
                 lambda: frozenset.symmetric_difference(set(), frozenset())):
        try:
            call()
        except TypeError:
            pass
        else:
            raise AssertionError("accepted invalid symmetric_difference call")


if __name__ == "__main__":
    current()
