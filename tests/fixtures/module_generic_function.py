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


def variadic[*Ts, **P](*args: *Ts, **kwargs: P.kwargs):
    return Ts, P


class MethodBase:
    def owner(self):
        return self


class MethodOwner(MethodBase):
    T = int

    def method[T, **__P](self, x: T = T) -> __class__:
        return T, __P, super().owner()


value = []
assert echo() is T
assert echo(value) is value
assert identity(value) is value

pack, signature = variadic.__type_params__
assert variadic() == (pack, signature)
assert variadic.__annotations__["args"].__args__ == (pack,)
assert variadic.__annotations__["kwargs"].__origin__ is signature


instance = MethodOwner()
method = MethodOwner.method
param, private = method.__type_params__
assert private.__name__ == "__P"
assert instance.method() == (param, private, instance)
assert method.__defaults__ == (int,)
assert method.__annotations__ == {"x": int, "return": MethodOwner}
