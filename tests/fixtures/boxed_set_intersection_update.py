def current():
    class Inherited(set):
        pass

    descriptor = Inherited.intersection_update
    assert descriptor is set.intersection_update
    assert type(descriptor).__name__ == "method_descriptor"
    child = Inherited([1, 2])
    assert descriptor.__get__(child)([2]) is None
    assert child.__contains__(2) and not child.__contains__(1)
    values = {1, 2, 3}
    alias = values
    update = values.intersection_update
    for method in (descriptor, update):
        assert method.__doc__ == "Update the set, keeping only elements found in it and all others."
        assert method.__text_signature__ == "($self, /, *others)"
    assert update() is None
    assert update([2, 3, 4], iter([1, 3, 3])) is None
    assert alias == {3}

    def partial():
        yield 2
        raise RuntimeError("broken source")

    values.update([1, 2])
    try:
        update([1, 2], partial())
    except RuntimeError as error:
        assert str(error) == "broken source"
    else:
        raise AssertionError("lost iteration error")
    assert values == {1, 2, 3}

    class Key:
        locked = False

        def __hash__(self):
            if self.locked:
                raise AssertionError("rehashed a stored key")
            return 42

        def __eq__(self, other):
            return True

    original = Key()
    equal = Key()
    receiver = {original}
    source = {equal}
    original.locked = equal.locked = True
    assert set.intersection_update(receiver, source) is None
    assert receiver.pop() is equal

    class Child(set):
        def intersection_update(self, other):
            return "override"

    child = Child([1, 2])
    assert child.intersection_update((2,)) == "override"
    assert set.intersection_update(child, (2,)) is None
    assert child.__contains__(2)
    assert not child.__contains__(1)
    assert not hasattr(frozenset(), "intersection_update")


if __name__ == "__main__":
    current()
