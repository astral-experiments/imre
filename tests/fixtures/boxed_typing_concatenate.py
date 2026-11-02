from collections import abc
import typing


P = typing.ParamSpec("P")
Q = typing.ParamSpec("Q")
T = typing.TypeVar("T")
prefix = typing.Concatenate[T, P]
assert type(prefix).__bases__ == (typing._GenericAlias,)
assert isinstance(prefix, typing._GenericAlias)
assert prefix.__name__ == prefix.__qualname__ == "Concatenate"
assert {"copy_with", "__origin__", "__args__", "__parameters__"} <= set(dir(prefix))
assert prefix.copy_with((int, [])) == (int,)
assert prefix.copy_with((str, typing.Concatenate[int, Q])) == typing.Concatenate[str, int, Q]
assert prefix.copy_with((str, bytes)).__args__ == (str, bytes)


class Origin:
    def public_member(arg):
        return arg

    def __origin_only__(arg):
        return arg


custom = type(prefix)(Origin, (int, P))
assert "public_member" in dir(custom) and "__origin_only__" not in dir(custom)
assert custom.public_member is Origin.public_member and custom.public_member(P) is P


assert P.__typing_subst__([None, bytes]) == (type(None), bytes)
assert typing.ParamSpec.__typing_prepare_subst__(P, prefix, (int, [bytes])) == (int, (bytes,))
for call, diagnostic in (
    (lambda: typing.ParamSpec.__typing_subst__([], P),
     "descriptor '__typing_subst__' for 'typing.ParamSpec' objects doesn't apply to a 'list' object"),
    (lambda: P.__typing_subst__(arg=[]), "ParamSpec.__typing_subst__() takes no keyword arguments"),
    (lambda: P.__typing_prepare_subst__(prefix), "__typing_prepare_subst__ expected 2 arguments, got 1"),
):
    try:
        call()
    except TypeError as error:
        assert str(error) == diagnostic
    else:
        raise AssertionError("invalid descriptor call was accepted")


for callable_form in (typing.Callable, abc.Callable):
    callback = callable_form[prefix, T]
    assert callback.__parameters__ == (T, P)
    assert typing.get_args(callback) == (prefix, T)
    assert repr(callback) == f"{callable_form.__module__}.Callable[typing.Concatenate[~T, ~P], ~T]"
    assert callback[int, [bytes]] == callable_form[[int, bytes], int]
    assert callback[int, Q] == callable_form[typing.Concatenate[int, Q], int]

try:
    prefix[int, float]
except TypeError as error:
    assert str(error) == f"Expected a list of types, an ellipsis, ParamSpec, or Concatenate. Got {float}"
else:
    raise AssertionError("a scalar was accepted for the ParamSpec")
