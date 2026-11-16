import typing


def current():
    T = typing.TypeVar("T")

    class C(typing.Generic[T]):
        pass

    def aliases():
        return typing.List[int], typing.Generic[T], typing.Callable[[str], int], C[int]

    calls = []

    @typing._tp_cache
    def token(value):
        calls.append(value)
        return object()

    @typing._tp_cache(typed=True)
    def types(left, right):
        return type(left), type(right)

    cached = token("same")
    assert token("same") is cached
    assert token([]) is not token([])
    assert calls == ["same", [], []]
    assert token.__name__ == token.__wrapped__.__name__ == "token"
    assert types(1, 2) == (int, int)
    assert types(1.0, 2) == (float, int)

    before = aliases()
    for first, repeated in zip(before, aliases()):
        assert first is repeated

    for clear in typing._cleanups:
        clear()

    assert token("same") is not cached

    for old, new, repeated in zip(before, aliases(), aliases()):
        assert old == new and old is not new
        assert new is repeated
        assert typing.get_origin(old) is typing.get_origin(new)
        assert typing.get_args(old) == typing.get_args(new)
    print("typing caches cleared")
