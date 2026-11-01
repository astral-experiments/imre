def current():
    suffix = "tools"
    functools = __import__("func" + suffix)
    lru_cache = functools.lru_cache
    events = []

    class Key:
        def __init__(self, name, code):
            self.name = name
            self.code = code

        def __hash__(self):
            return self.code

        def __eq__(self, other):
            events.append((self.name, other.name))
            return self.name == other.name

    @lru_cache
    def cached(*args, **kwargs):
        events.append("call")
        return len(events)

    print(cached(Key("a", 1)), cached(Key("a", 1)))
    print(cached(Key("a", 2)))
    print(events)
    events.clear()
    cached(Key("x", 1), Key("y", 2))
    cached(Key("z", 3), Key("w", 4421810932546178461))
    print(events)
    events.clear()
    print(cached(Key("k", 4), named=Key("v", 5)))
    print(cached(Key("k", 4), named=Key("v", 5)))
    print(events)

    unbounded = functools.cache(len)
    assert unbounded((1,)) == 1
    assert unbounded.cache_info() == (0, 1, None, 1)
    assert unbounded.cache_parameters() == {"maxsize": None, "typed": False}

    for limit, resident in ((None, "outer"), (2, "inner")):
        calls = []

        @lru_cache(maxsize=limit)
        def recursive(x):
            calls.append(x)
            if len(calls) == 1:
                assert recursive(x) == "inner"
                return "outer"
            return "inner"

        assert recursive(42) == "outer"
        assert recursive(42) == resident
        assert recursive.cache_info() == (1, 2, limit, 1)

    for limit in (0, -2):
        calls = []

        @lru_cache(limit)
        def disabled(x):
            calls.append(x)
            return len(calls)

        assert disabled([]) == 1 and disabled([]) == 2
        assert disabled.cache_info() == (0, 2, 0, 0)
        assert disabled.cache_parameters() == {"maxsize": 0, "typed": False}

    @lru_cache(2)
    def small(x):
        return object()

    first, second = small(1), small(2)
    assert small(1) is first
    small(3)
    assert small(2) is not second
    assert small.cache_info() == (1, 4, 2, 2)

    for typed, expected in ((True, 2), (False, 1)):
        calls = []

        @lru_cache(maxsize=32768, typed=typed)
        def identify(*args, **kw):
            calls.append(None)
            return len(calls)

        assert identify(1, x=1) == 1
        assert identify(1.0, x=1) == expected
        assert identify(1, x=1.0) == (3 if typed else 1)
        assert identify((1,)) == identify((1.0,))

    marker = []
    factory = lru_cache(typed=marker)
    metadata = factory(user_function=len).cache_parameters
    assert metadata()["typed"] is marker
    assert metadata() is not metadata() and metadata is not factory(len).cache_parameters
    for invalid in (lambda: lru_cache("bad"), lambda: lru_cache(len, typed=1),
                    lambda: functools.cache(user_function=len),
                    lambda: lru_cache(1, maxsize=2), lambda: lru_cache(1)(None)):
        try:
            invalid()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid cache arguments accepted")
