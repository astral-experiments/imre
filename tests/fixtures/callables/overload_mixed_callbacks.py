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


def check() -> None:
    print(invoke(Reader().fetch))
    callbacks = get_overloads(read)
    print(len(callbacks))
    print(callbacks[0].__qualname__)
    recovered: Callable[[], int] = callbacks[0]
    clear_overloads()
    print(invoke(recovered))
    print(read())


check()
