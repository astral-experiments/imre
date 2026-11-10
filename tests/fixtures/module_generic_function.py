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


def make_converter(default):
    from functools import wraps

    def convert[T](value: T = default) -> T:
        return value

    @wraps(convert)
    def logged(*args):
        return convert(*args)

    return logged


def declarations():
    current = int

    attempts = []

    def bound():
        attempts.append(None)
        if len(attempts) == 1:
            raise LookupError("bound")
        return current

    def child[T: bound() = current]():
        return T

    yield child
    assert len(attempts) == 2
    current = str
    yield current


value = []
assert echo() is T
assert echo(value) is value
assert identity(value) is value

first = make_converter("missing")
second = make_converter(0)
parameter, = first.__wrapped__.__type_params__
assert first.__type_params__ is first.__wrapped__.__type_params__
assert second.__type_params__[0] is not parameter
assert first.__annotations__ == {"value": parameter, "return": parameter}
assert first() == "missing" and first(42) == 42 and second() == 0


declared = declarations()
child = next(declared)
parameter, = child.__type_params__
assert child() is parameter
try:
    parameter.__bound__
except LookupError as exc:
    assert str(exc) == "bound"
else:
    raise AssertionError("expected the first bound evaluation to fail")
assert parameter.__bound__ is int
assert next(declared) is str
assert parameter.__default__ is str
assert parameter.__bound__ is int and parameter.evaluate_bound() is str
