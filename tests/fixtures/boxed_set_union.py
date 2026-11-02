"""Method-set setup in Flask mutates a set retained by its caller."""


def current():
    class Child(set):
        pass

    values = Child((1,))
    values.add(2)
    values.update(Child((3,)))
    assert values.__contains__(2)
    assert values.issuperset((1, 2, 3))

    class Override(set):
        def update(self, *args):
            raise AssertionError("called subclass update")
        def add(self, item):
            raise AssertionError("called subclass add")

    target = Override([1])
    update = set.update
    assert update(target, (2,), {3}) is None
    assert target == {1, 2, 3}
    assert Child.update(target, target) is None
    assert update.__get__(target, set)([4]) is None
    assert target == {1, 2, 3, 4}
    for args, kwargs in (((), {}), ((frozenset(),), {}), ((target,), {"items": ()})):
        try:
            update(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError("invalid set.update call")

    for kind in (set, frozenset):
        original = kind([1])
        combine = original.union
        assert combine() == original and combine() is not original
        assert kind.union(original, (2,), iter([3]), {4: None}) == {1, 2, 3, 4}
        assert type(combine(original)) is kind
        try:
            combine([5], [[]])
        except TypeError:
            assert original == {1}
        else:
            raise AssertionError("unhashable union element accepted")

    class Key:
        def __hash__(self):
            if self.locked:
                raise AssertionError("rehashing an exact dict key")
            return 17

    key = Key()
    key.locked = False
    mapping = {key: None}
    key.locked = True
    assert list(set().union(mapping)) == [key]

    events = []
    class EqualKey:
        def __init__(self, name):
            self.name = name
        def __hash__(self):
            return 1
        def __eq__(self, other):
            return True
        def __del__(self):
            events.append(self.name)

    def duplicates():
        yield EqualKey("duplicate")
        assert events == ["duplicate"]

    resident = EqualKey("resident")
    assert {resident}.union(duplicates()) == {resident}

    for left_type in (set, frozenset):
        for right_type in (set, frozenset):
            left = left_type(["GET"])
            right = right_type(["GET", "OPTIONS"])
            result = left | right
            print(type(result).__name__, result == {"GET", "OPTIONS"}, result is left)
            before = left
            left |= right
            print(left == result, left is before, before == result, right == result)
            before = left
            left |= left
            print(left == before, left is before)

    marker = []
    class Reflected:
        def __ror__(self, other):
            return marker
    original = {"GET"}
    assigned = original
    assigned |= Reflected()
    print(assigned is marker, original == {"GET"})
    print((original | Reflected()) is marker)
    for invalid in ([], ("OPTIONS",), {"OPTIONS": None}, 4):
        try:
            original |= invalid
        except TypeError as error:
            print(error.args, original == {"GET"})

    class Key:
        def __init__(self, name):
            self.name = name
            self.calls = 0
            self.reject = False

        def __hash__(self):
            self.calls += 1
            if self.calls > 1:
                raise AssertionError("hashed an existing set element")
            return 43

        def __eq__(self, other):
            if self.reject:
                raise ValueError("collision")
            return self.name == other.name

    left_key = Key("equal")
    right_key = Key("equal")
    left = {left_key}
    right = {right_key}
    left |= right
    print(len(left), list(left)[0] is left_key, left_key.calls, right_key.calls)
    copied = set(right)
    print(len(copied), list(copied)[0] is right_key)
    left_key.reject = True
    try:
        left |= right
    except ValueError as error:
        print(error.args, len(left), left_key.calls, right_key.calls)
    frozen = frozenset(right)
    print(len({frozen: True}), right_key.calls)

    class Mutating:
        def __hash__(self):
            return 17

        def __eq__(self, other):
            for i in range(16):
                target.add(i)
            return True

    resident = Mutating()
    incoming = Mutating()
    target = {resident}
    target |= {incoming}
    print(len(target), resident in target, 15 in target)

    class Holder:
        pass
    holder = Holder()
    holder.methods = {"GET"}
    held = holder.methods
    holder.methods |= {"OPTIONS"}
    slots = [held]
    slots[0] |= {"HEAD"}
    print(held is holder.methods, held is slots[0], len(held))
    expected = []
    alias = expected
    expected += ["item"]
    print(expected is alias, alias)

    for view in ({"right": 9}.keys(), {"right": 9}.items()):
        left = frozenset(["left"])
        before = left
        left |= view
        print(type(left).__name__, len(left), before is left, left == view | before)
    left_key = Key("nested")
    right_key = Key("nested")
    left = {frozenset([left_key])}
    right = {frozenset([right_key])}
    left |= right
    print(len(left), left_key.calls, right_key.calls)
