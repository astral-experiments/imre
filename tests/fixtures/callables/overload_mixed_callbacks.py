from typing import Callable, clear_overloads, get_overloads, overload


@overload
def read() -> int:
    return 17


def read() -> int:
    return 19


class Reader:
    def fetch(self) -> int:
        return 23


def invoke(callback: Callable[[], int]) -> int:
    return callback()


def register(callback: Callable[[], int]) -> None:
    alias = callback
    overload(alias)


def check() -> None:
    print(invoke(Reader().fetch))
    overload(sum)
    overload(print)
    assert get_overloads(sum) == []
    assert [] == get_overloads(print)
    callbacks = get_overloads(read)
    assert callbacks != []
    assert [] != callbacks
    print(len(callbacks))
    print(callbacks[0].__qualname__)
    recovered: Callable[[], int] = callbacks[0]
    clear_overloads()
    print(invoke(recovered))
    print(read())
    register(read)
    print(len(get_overloads(read)))
    clear_overloads()


check()
