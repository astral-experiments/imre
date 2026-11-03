import contextlib


class Guard:
    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


def plus_one(value: int) -> int:
    return value + 1


def call_decorated(value: int) -> int:
    class Around(Guard, contextlib.ContextDecorator):
        pass

    return Around()(plus_one)(value)


assert call_decorated(41) == 42
