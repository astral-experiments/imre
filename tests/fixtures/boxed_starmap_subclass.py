import itertools

def current():
    events = []
    class Source:
        def __iter__(self):
            events.append("iter")
            return iter([(2, 3), (3, 2)])

    class Init(itertools.starmap):
        def __init__(self, *args, marker):
            events.append(("init", marker))
            self.marker = marker

    class New(itertools.starmap):
        def __new__(cls, *args, marker):
            events.append(("new", marker))
            result = super().__new__(cls, *args)
            result.marker = marker
            return result

    for cls, expected in ((Init, ["iter", ("init", 41)]), (New, [("new", 41), "iter"])):
        events.clear()
        x = cls(pow, Source(), marker=41)
        assert type(x) is cls and x.marker == 41
        assert events == expected
        assert iter(x) is x and next(x) == 8
        # A heap subclass can use these public-looking names for unrelated state.
        x._function = None
        x._source = None
        assert list(x) == [9] and next(x, "end") == "end"

    class Plain(itertools.starmap):
        pass
    events.clear()
    try:
        Plain(pow, Source(), rejected=True)
    except TypeError as error:
        assert str(error) == "starmap() takes no keyword arguments"
    else:
        raise AssertionError("subclass without initializer accepted arbitrary keys")
    assert events == []
    raw = itertools.starmap.__new__(Init, pow, Source(), marker=19)
    assert type(raw) is Init and not hasattr(raw, "marker")
    assert next(raw) == 8
    raw.__init__(marker=19)
    assert raw.marker == 19 and next(raw) == 9
    for cls in (Plain, Init):
        try:
            object.__new__(cls)
        except TypeError:
            pass
        else:
            raise AssertionError("unsafe bare allocation created an uninitialized cursor")
    base = itertools.starmap(pow, [(4, 2)])
    itertools.starmap.__init__(base, None, None)
    assert next(base) == 16

    class Row(tuple):
        def __iter__(self):
            return iter((2, 5))

    cursor = itertools.starmap(pow, [Row((3, 4)), None, iter((2, 3))])
    assert next(cursor) == 32
    try:
        next(cursor)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a non-iterable argument row")
    assert next(cursor) == 8

    # Reimporting the module creates another class. Existing instances and
    # constructors must keep referring to their original native layout.
    import sys
    old = itertools.starmap
    existing = old(pow, [(3, 2)])
    saved_module = sys.modules.pop("itertools")
    try:
        fresh = __import__("itertools").starmap
        assert next(existing) == 9
        assert list(old(pow, [(4, 2)])) == [16]
        assert list(fresh(pow, [(2, 5)])) == [32]
        for owner in (old, fresh):
            try:
                object.__new__(owner)
            except TypeError:
                pass
            else:
                raise AssertionError("unsafe bare allocation of a starmap generation")
    finally:
        sys.modules["itertools"] = saved_module
