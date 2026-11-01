import functools


def current():
    seen = []

    def work(*args, **kwargs):
        "preserved document"
        seen.append((args, kwargs))
        if args == ('error',):
            raise ValueError('failed')
        return len(seen)

    work.tag = 731
    cached = functools.cache(work)
    assert cached.__name__ == work.__name__
    assert cached.__doc__ == work.__doc__
    assert cached.__wrapped__ is work
    assert cached.tag == 731
    assert cached.cache_parameters() == {'maxsize': None, 'typed': False}
    assert cached.cache_parameters() is not cached.cache_parameters()
    assert cached(1) == 1
    for n in range(2, 161):
        assert cached(n) == n
    assert cached(1) == 1
    assert cached(x=1, y=2) == 161
    assert cached(x=1, y=2) == 161
    assert cached(y=2, x=1) == 162
    assert cached(1.0) == 163
    assert cached(True) == 163
    assert cached(1) == 1
    for _ in range(2):
        try:
            cached('error')
        except ValueError as e:
            assert str(e) == 'failed'
        else:
            raise AssertionError('lost exception')
    try:
        cached([])
    except TypeError:
        pass
    else:
        raise AssertionError('unhashable accepted')
    info = cached.cache_info()
    assert (info.hits, info.misses, info.maxsize, info.currsize) == (4, 165, None, 163)
    assert info == (4, 165, None, 163)
    assert cached.__wrapped__(1) == 166
    assert cached(1) == 1
    assert cached.cache_clear() is None
    assert cached.cache_info() == (0, 0, None, 0)
    assert cached(1) == 167

    @functools.lru_cache(maxsize=None)
    def other(k):
        return k

    assert other(7) == 7
    assert other.cache_info() == (0, 1, None, 1)

    class C:
        @functools.cache
        def f(self, x):
            return x + 2

    c = C()
    assert c.f(3) == 5
    assert c.f(3) == 5
    assert C.f.cache_info() == (1, 1, None, 1)
    assert c.f.__func__ is C.f
    assert c.f.cache_info() == (1, 1, None, 1)
    assert type(cached).__name__ == '_lru_cache_wrapper'
    assert type(cached).__module__ == 'functools'
    assert isinstance(cached, type(cached))
    assert callable(functools.total_ordering)
    calls = []

    @functools.cache
    def recursive(k):
        calls.append(k)
        if len(calls) == 1:
            return recursive(k) + 1
        return 10

    assert recursive(1) == 11 and recursive(1) == 11
    assert recursive.cache_info() == (1, 2, None, 1)

    @functools.cache
    def clearing(k):
        clearing.cache_clear()
        return k

    assert clearing(2) == 2
    assert clearing.cache_info() == (0, 0, None, 1)
    print('unbounded cache controls passed')
