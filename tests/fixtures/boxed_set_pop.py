def current():
    class Key:
        locked = False

        def __hash__(self):
            if self.locked:
                raise AssertionError("pop rehashed a stored key")
            return 17

    key = Key()
    values = {key}
    pop = values.pop
    key.locked = True
    assert pop() is key and not values
    try:
        pop()
    except KeyError as error:
        assert error.args == ("pop from an empty set",)
    else:
        raise AssertionError("popped an empty set")

    class Subset(set):
        def pop(self):
            return "override"

    values = Subset((1, 2, 3))
    assert values.pop() == "override"
    assert {set.pop(values) for _ in range(3)} == {1, 2, 3} and not values
    assert not hasattr(frozenset(), "pop")

    values = {42}
    for args, kwargs, message in (
        ((), {}, "unbound method set.pop() needs an argument"),
        ((frozenset(),), {}, "descriptor 'pop' for 'set' objects doesn't apply to a 'frozenset' object"),
        ((values, 1), {}, "set.pop() takes no arguments (1 given)"),
        ((values,), {"key": 1}, "set.pop() takes no keyword arguments"),
    ):
        try:
            set.pop(*args, **kwargs)
        except TypeError as error:
            assert str(error) == message
        else:
            raise AssertionError("invalid call succeeded")
    assert values == {42}


if __name__ == "__main__":
    current()
