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
