def current():
    data = {1, 2, 3, 4}
    ref = data
    print(data.difference_update(), data.difference_update((1,), [2, 2]), sorted(ref))
    print(set.difference_update(data, data), sorted(ref))
    data.update([1, 2, 3])

    def incomplete():
        yield 1
        raise RuntimeError("iterator broke")

    try:
        data.difference_update({2}, incomplete(), {3})
    except RuntimeError as exc:
        print(str(exc), sorted(ref))

    class Key:
        def __init__(self):
            self.calls = 0

        def __hash__(self):
            self.calls += 1
            return 19

    key = Key()
    lhs = {key, 4}
    rhs = frozenset([key])
    calls = key.calls
    lhs.difference_update(rhs)
    print(lhs == {4}, key.calls == calls)

    for call in (
        lambda: set.difference_update(),
        lambda: set.difference_update(7, ()),
        lambda: set.difference_update(set(), bad=()),
        lambda: set().difference_update([[]]),
    ):
        try:
            call()
        except TypeError as exc:
            print(type(exc).__name__, str(exc))

    class Child(set):
        def __iter__(self):
            raise RuntimeError("should use stored set members")

    child = Child([0, 1])
    set.difference_update(child, Child(range(32)))
    print(not child.__contains__(0), not child.__contains__(1))
