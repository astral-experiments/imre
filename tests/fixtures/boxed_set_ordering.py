def current():
    for left_kind in (set, frozenset):
        for right_kind in (set, frozenset):
            left = left_kind([1])
            equal = right_kind([1])
            larger = right_kind([1, 2])
            disjoint = right_kind([3])
            assert left <= equal and left >= equal
            assert not left < equal and not left > equal
            assert left < larger and larger > left
            assert not left <= disjoint and not left >= disjoint

    class Reflected(set):
        def __ge__(self, other):
            return "reflected"

    assert ({1} <= Reflected([2])) == "reflected"
    assert (frozenset([1]) <= Reflected([1])) is True

    class Mixin:
        def __le__(self, other):
            return "mixin"

    class SetFirst(set, Mixin):
        pass

    class MixinFirst(Mixin, set):
        pass

    assert (SetFirst([1]) <= {2}) is False
    assert (MixinFirst([1]) <= {2}) == "mixin"
    try:
        SetFirst([1]) <= [1]
    except TypeError:
        pass
    else:
        raise AssertionError("late mixin replaced native set ordering")

    class Key:
        def __init__(self):
            self.hashed = False

        def __hash__(self):
            assert not self.hashed, "ordering rehashed a member"
            self.hashed = True
            return 1

        def __eq__(self, other):
            raise ValueError("member comparison")

    left = {Key()}
    right = frozenset([Key()])
    assert not left < right
    try:
        left <= right
    except ValueError as error:
        assert error.args == ("member comparison",)
    else:
        raise AssertionError("lost member comparison error")
    print("ok")
