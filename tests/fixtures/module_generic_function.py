T = object()


def inspect_definition(fn):
    param, = fn.__type_params__
    assert param is not T
    assert fn.__defaults__[0] is T
    assert fn.__annotations__ == {"value": param, "return": param}
    return fn


@inspect_definition
def echo[T](value: T = T, /) -> T:
    assert T is echo.__type_params__[0]
    return value


def identity[int](value: int, /) -> int:
    return value


value = []
assert echo() is T
assert echo(value) is value
assert identity(value) is value
