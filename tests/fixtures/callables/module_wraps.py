from functools import wraps as preserve


def original(value: int) -> int:
    """Wrapped function."""
    return value + 1


@preserve(original)
def wrapper(value: int) -> int:
    return 2 * original(value)


@preserve(wrapper)
def outer(value: int) -> int:
    return wrapper(value) + 1


print(wrapper(3), wrapper.__wrapped__ is original, wrapper.__name__, wrapper.__doc__)
assert outer.__wrapped__ is wrapper
assert outer.__wrapped__.__wrapped__ is original
assert outer.__wrapped__(3) == 8
assert outer(3) == 9
