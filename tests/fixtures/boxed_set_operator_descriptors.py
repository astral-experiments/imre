class Subset(set):
    def __sub__(self, other):
        return "override"

    def __iter__(self):
        raise AssertionError("base set slot called __iter__")


class Frozen(frozenset):
    def __ror__(self, other):
        return "reflected override"


def current():
    left = Subset([1, 2])
    right = Frozen([2, 3])
    assert left.__sub__(right) == "override"
    assert right.__ror__(left) == "reflected override"

    for owner, receiver, operand in ((set, left, right), (frozenset, right, left)):
        for name, symbol, expected, reversed_expected in (
            ("sub", "-", {1} if owner is set else {3}, {3} if owner is set else {1}),
            ("and", "&", {2}, {2}),
            ("or", "|", {1, 2, 3}, {1, 2, 3}),
            ("xor", "^", {1, 3}, {1, 3}),
        ):
            for prefix, answer in (("", expected), ("r", reversed_expected)):
                slot_name = "__" + prefix + name + "__"
                slot = getattr(owner, slot_name)
                assert slot.__objclass__ is owner
                assert type(slot).__name__ == "wrapper_descriptor"
                assert slot.__text_signature__ == "($self, value, /)"
                expression = "value" + symbol + "self" if prefix else "self" + symbol + "value"
                assert slot.__doc__ == "Return " + expression + "."
                result = slot(receiver, operand)
                result_type = (frozenset if owner is set else set) if prefix else owner
                assert type(result) is result_type and result == answer
                assert slot.__get__(receiver)(operand) == answer
                assert slot(receiver, [1]) is NotImplemented
                assert slot(receiver, {1: 2}.keys()) is NotImplemented
                assert slot(receiver, receiver) is not receiver
                for call in (lambda: slot(), lambda: slot(operand, receiver),
                             lambda: slot(receiver), lambda: slot(receiver, operand, operand),
                             lambda: slot(receiver, value=operand), lambda: slot.__get__(operand)):
                    try:
                        call()
                    except TypeError as error:
                        print(error)
                    else:
                        raise AssertionError("invalid set operator call accepted")

    assert left == {1, 2} and right == {2, 3}
    for name, expected in (("__iand__", {2}), ("__ior__", {1, 2, 3}), ("__isub__", {1}), ("__ixor__", {1, 3})):
        target = Subset([1, 2])
        slot = getattr(set, name)
        assert slot(target, [2]) is NotImplemented
        assert slot(target, right) is target and target == expected
        assert not hasattr(frozenset, name)
    target = {1}
    alias = target
    target |= right
    assert target is alias and target == {1, 2, 3}

    class Key:
        def __init__(self, label, equal):
            self.label = label
            self.equal = equal

        def __hash__(self):
            return 991

        def __eq__(self, other):
            events.append(self.label)
            if self.equal is error:
                raise error
            return self.equal

    events = []
    error = ValueError("resident equality")
    a, b = Key("left", True), Key("right", False)
    other = frozenset([b])
    for size, expected_events in ((1, ["right"]), (7, ["right"]), (8, ["left"])):
        items = set(range(size - 1))
        items.add(a)
        for subtract in (set.__sub__, lambda x, y: x - y, lambda x, y: frozenset.__rsub__(y, x)):
            events.clear()
            result = subtract(items, other)
            assert events == expected_events
            assert any(item is a for item in result) == (size < 8)
        events.clear()
        assert set.__isub__(items, other) is items
        assert events == ["left"] and len(items) == size - 1

    b.equal = error
    try:
        set.__sub__({a}, other)
    except ValueError as exc:
        assert exc is error
    else:
        raise AssertionError("right-hand comparison exception lost")

    a.equal, b.equal = False, False
    for owner in (set, frozenset):
        items = owner([a, b])
        a.equal = b.equal = error
        events.clear()
        result = owner.__and__(items, items)
        assert type(result) is owner and result is not items and len(result) == 2
        if owner is set:
            assert set.__iand__(items, items) is items and len(items) == 2
        assert events == []
        a.equal = b.equal = False

    class Collision:
        def __hash__(self):
            return 7

        def __eq__(self, other):
            raise ValueError("colliding keys")

    for slot in (set.__sub__, set.__or__, set.__and__, set.__xor__):
        try:
            slot({Collision()}, {Collision()})
        except ValueError as error:
            assert str(error) == "colliding keys"
        else:
            raise AssertionError("set operator swallowed a key comparison error")
