import typing


def current():
    T = typing.TypeVar("T")

    def aliases():
        return typing.List[int], typing.Generic[T], typing.Callable[[str], int]

    before = aliases()
    for first, repeated in zip(before, aliases()):
        assert first is repeated

    for clear in typing._cleanups:
        clear()

    for old, new, repeated in zip(before, aliases(), aliases()):
        assert old == new and old is not new
        assert new is repeated
        assert typing.get_origin(old) is typing.get_origin(new)
        assert typing.get_args(old) == typing.get_args(new)
    print("typing caches cleared")
