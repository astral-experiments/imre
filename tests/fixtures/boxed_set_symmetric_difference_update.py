def current():
    values = {1, 2}
    update = values.symmetric_difference_update
    assert update(iter([2, 3, 3])) is None
    assert values == {1, 3}

    def partial():
        yield 1
        raise RuntimeError("broken source")

    try:
        update(partial())
    except RuntimeError as error:
        assert str(error) == "broken source"
    else:
        raise AssertionError("lost iteration error")
    assert values == {1, 3}

    class Key:
        locked = False

        def __hash__(self):
            if self.locked:
                raise AssertionError("rehashed a stored key")
            return 42

    key = Key()
    source = {key: None}
    bucket = set()
    difference = {key}
    key.locked = True
    try:
        difference.difference_update(source)
    except AssertionError as error:
        assert str(error) == "rehashed a stored key"
    else:
        raise AssertionError("difference_update reused the dictionary hash")
    assert difference.pop() is key
    assert set.symmetric_difference_update(bucket, source) is None
    assert bucket.pop() is key

    class Child(set):
        def symmetric_difference_update(self, other):
            return "override"

    child = Child([7])
    assert child.symmetric_difference_update(child) == "override"
    assert child.__contains__(7)
    assert set.symmetric_difference_update(child, child) is None
    assert not child.__contains__(7)
    assert not hasattr(frozenset(), "symmetric_difference_update")


if __name__ == "__main__":
    current()
