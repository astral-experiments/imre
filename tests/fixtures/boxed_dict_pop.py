class Mapping(dict):
    def __missing__(self, key):
        raise AssertionError("pop called __missing__")

    def __delitem__(self, key):
        raise AssertionError("pop called __delitem__")


def current():
    value = [7]
    data = Mapping(a=value, b=2)
    keys = data.keys()
    print(data.pop("a") is value, list(keys))
    print(dict.pop(data, "missing", value) is value, list(keys))
    print(data.pop("b", None), list(keys))
    for key in ("missing", 7, (1, 2)):
        try:
            data.pop(key)
        except KeyError as error:
            print(error.args, error.args[0] is key)
        try:
            del {}[key]
        except KeyError as error:
            print(error.args, error.args[0] is key)
    print(data.pop([], value) is value)
    try:
        {"a": 1}.pop((0, []), None)
    except TypeError as error:
        print(str(error))
    for method, args in (
        (dict.pop, ()),
        (dict.pop, ([], 1)),
        (data.pop, ()),
        (data.pop, (1, 2, 3)),
    ):
        try:
            method(*args)
        except TypeError as error:
            print(error.args)
    try:
        data.pop("missing", default=None)
    except TypeError as error:
        print(error.args)

    # Removal bypasses Python lookup even when colliding stored keys refuse comparison.
    first, last = StoredKey(), StoredKey()
    data = Mapping()
    data[first] = 1
    data[last] = value
    data[first] = 2
    visible = data.keys()
    live = iter(data)
    first.locked = last.locked = True
    key, item = dict.popitem(data)
    assert key is last and item is value and len(visible) == 1
    try:
        next(live)
    except RuntimeError as error:
        print(error.args)
    else:
        raise AssertionError("iterator missed dictionary size change")
    key, item = data.popitem()
    assert key is first and item == 2
    for target, args, kwargs in (
        (data.popitem, (), {}), ({}.popitem, (1,), {}), ({}.popitem, (), {"last": True}),
        (dict.popitem, (), {}), (dict.popitem, (None,), {}),
    ):
        try:
            target(*args, **kwargs)
        except (KeyError, TypeError) as error:
            print(type(error).__name__, error.args)
        else:
            raise AssertionError("invalid dict.popitem call succeeded")
    bound = data.popitem
    for args, kwargs, reason in (((1,), {}, "takes no arguments (1 given)"),
                                 ((), {"last": True}, "takes no keyword arguments")):
        try:
            bound(*args, **kwargs)
        except TypeError as error:
            assert str(error).endswith(reason)
        else:
            raise AssertionError("bound subclass popitem accepted arguments")


class StoredKey:
    locked = False

    def __hash__(self):
        if self.locked:
            raise AssertionError("popitem rehashed its stored key")
        return 17

    def __eq__(self, other):
        if self.locked:
            raise AssertionError("popitem compared its stored key")
        return self is other
