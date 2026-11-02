from itertools import count, repeat


def replace_next(mapping):
    mapping["__next__"] = None


def remove_next(mapping):
    del mapping["__next__"]


def rejects(call, *args):
    try:
        call(*args)
    except TypeError:
        return
    raise AssertionError("expected TypeError")


def current():
    slots = ("__repr__", "__getattribute__", "__iter__", "__next__")
    for cls, args in ((count, (3, 2)), (repeat, ("x", 2))):
        methods = slots + (("__length_hint__",) if cls is repeat else ())
        keys = ("__new__",) + methods + ("__doc__", "__module__")
        mapping = cls.__dict__
        other = cls.__dict__
        assert type(mapping).__name__ == "mappingproxy" and mapping is not other
        for method in ("keys", "values", "items", "get", "copy"):
            bound = getattr(mapping, method)
            assert bound.__self__ is mapping
            rejects(getattr(type(mapping), method), {})
        assert mapping.get("__next__") is cls.__next__
        copy = mapping.copy()
        copy.clear()
        assert "__next__" in mapping
        assert tuple(mapping) == keys and tuple(name for name, entry in mapping.items()) == keys
        assert "__dict__" not in mapping and "__init__" not in other
        for key, entry in mapping.items():
            assert other[key] is entry and getattr(cls, key) is entry
        assert mapping["__module__"] == "itertools" and mapping["__doc__"]
        new = mapping["__new__"]
        assert type(new).__name__ == "builtin_function_or_method" and new.__self__ is cls
        assert new.__name__ == "__new__" and new.__qualname__ == cls.__name__ + ".__new__"
        cursor = new(cls, *args)
        assert type(cursor) is cls
        assert set(dir(cls)) == set(dir(object)) | set(keys)
        assert dir(cursor) == dir(cls) and tuple(vars(cls)) == keys
        rejects(new, object, *args)
        rejects(new)

        for key in methods:
            entry = mapping[key]
            kind = "method_descriptor" if key == "__length_hint__" else "wrapper_descriptor"
            assert type(entry).__name__ == kind and entry.__objclass__ is cls
            assert entry.__name__ == key and entry.__qualname__ == cls.__name__ + "." + key
            assert entry.__get__(None, cls) is entry
            assert entry.__get__(cursor, cls).__self__ is cursor
        assert mapping["__getattribute__"](cursor, "__class__") is cls
        assert mapping["__iter__"](cursor) is cursor
        assert mapping["__repr__"](cursor) == repr(cursor)
        rejects(mapping["__next__"], object())
        rejects(mapping["__next__"].__get__, object(), object)
        rejects(replace_next, mapping)
        rejects(remove_next, mapping)
        rejects(setattr, cls, "extra", 1)
        rejects(delattr, cls, "__next__")
        assert other["__next__"] is cls.__next__ and "extra" not in mapping

        if cls is count:
            assert mapping["__next__"](cursor) == 3
            assert cursor.__next__() == 5 and repr(cursor) == "count(7, 2)"
        else:
            hint = mapping["__length_hint__"]
            rejects(hint, object())
            assert hint(cursor) == 2 and mapping["__next__"](cursor) is args[0]
            assert cursor.__length_hint__() == 1 and next(cursor) is args[0]
            assert hint(cursor) == 0 and next(cursor, None) is None
        print(cls.__name__, keys)
