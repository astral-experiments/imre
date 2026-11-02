from collections import defaultdict
from types import SimpleNamespace


def current():
    left_value = object()
    right_value = object()
    left = {"left": left_value, "shared": left_value}
    right = SimpleNamespace(shared=right_value, right=right_value).__dict__
    merged = left | right
    assert type(merged) is dict and list(merged) == ["left", "shared", "right"]
    assert merged["shared"] is right_value and merged["left"] is left_value
    del merged["left"]
    assert left["left"] is left_value and left["shared"] is left_value
    assert right == {"shared": right_value, "right": right_value}
    assert (left | left) is not left

    class Right(dict):
        def __ror__(self, other):
            return other

    assert left | Right() is left

    class Defaults(dict):
        def __getitem__(self, key):
            raise AssertionError("union with inherited dict iteration must use stored values")

    defaults = Defaults(shared=left_value, fallback=left_value)
    merged = defaults | right
    assert type(merged) is dict and list(merged) == ["shared", "fallback", "right"]
    assert merged["shared"] is right_value and merged["fallback"] is left_value
    assert type(left | defaults) is dict and defaults | Right() == dict(defaults)
    assert dict.__ror__(defaults, right)["shared"] is left_value
    assert dict.__or__(defaults, []) is NotImplemented

    class DeclinesUnion(dict):
        def __or__(self, other):
            return NotImplemented

    declined = DeclinesUnion(left)
    assert declined | right == left | right
    try:
        declined | declined
    except TypeError:
        pass
    else:
        raise AssertionError("identical types must not retry the reflected union")

    auto = defaultdict(list, shared=left_value)
    for merged in (auto | right, right | auto):
        assert type(merged) is defaultdict and merged.default_factory is list
        assert merged["missing"] == [] and "missing" not in auto
    assert (auto | right)["shared"] is right_value and (right | auto)["shared"] is left_value
    assert type(dict.__or__(auto, right)) is dict

    class Target(dict):
        def __setitem__(self, key, value):
            raise AssertionError("dict union must write to its own storage")

    class Mapping:
        def keys(self):
            return ["new", "stop"]

        def __getitem__(self, key):
            if key == "stop":
                raise LookupError(key)
            return right_value

        def __ror__(self, other):
            raise AssertionError("in-place dictionary union tried a reflected hook")

    original = Target(shared=left_value)
    alias = original
    original |= [("new", left_value), ("shared", right_value)]
    assert original is alias and list(alias) == ["shared", "new"]
    assert alias["shared"] is right_value and alias["new"] is left_value
    try:
        original |= Mapping()
    except LookupError as error:
        assert error.args == ("stop",)
    else:
        raise AssertionError("in-place union suppressed the mapping error")
    assert original is alias and original["new"] is right_value
    try:
        dict.__ior__(original, [("first", 1), (2,)])
    except ValueError as error:
        assert "element #1 has length 1" in str(error)
    else:
        raise AssertionError("in-place union accepted a malformed pair")
    assert original["first"] == 1
    assert dict.__ior__(original, original) is original

    class Declines(dict):
        def __ior__(self, other):
            return NotImplemented

    class Reflects:
        def __ror__(self, other):
            return (other, self)

    declines = saved = Declines()
    right = Reflects()
    declines |= right
    assert declines[0] is saved and declines[1] is right and not saved
