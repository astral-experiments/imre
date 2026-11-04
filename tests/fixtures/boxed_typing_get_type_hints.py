from annotationlib import Format, ForwardRef
from collections.abc import Callable
from dataclasses import dataclass
from types import GenericAlias
from typing import Annotated, ReadOnly, Unpack, get_args, get_origin, get_type_hints


@dataclass
class Handler:
    callback: Callable[[Annotated["Target", "input"], int], "Target"]


class Target:
    pass


class Parent:
    item: "Target"
    values: list["Target"]


class Child(Parent):
    item: bytes


def fn(a: "Target") -> None:
    pass


def raise_user_error():
    error = NameError("user callback")
    error.name = "callback_name"
    raise error


def error_for(expression):
    def annotated():
        pass

    annotated.__annotations__ = {"value": expression}
    try:
        get_type_hints(annotated, {"Target": Target, "callback": raise_user_error})
    except (NameError, AttributeError) as error:
        return error
    raise AssertionError("annotation unexpectedly resolved")


def current():
    class Owner[Param]:
        item: ForwardRef("Param", module="builtins")
        items: list[ForwardRef("Param", module="builtins")]

    param, = Owner.__type_params__
    assert get_type_hints(Owner) == {"item": param, "items": list[param]}
    # The explicit local binding wins when _hint_scope retains it.
    assert get_type_hints(Owner, {"Param": str}, {"Param": bytes})["item"] is bytes

    assert get_type_hints(Parent) == {"item": Target, "values": list[Target]}
    assert get_type_hints(Child) == {"item": bytes, "values": list[Target]}
    plain = get_type_hints(Handler)["callback"]
    extra = get_type_hints(Handler, include_extras=True)["callback"]
    assert type(plain) is type(extra) is GenericAlias
    assert get_origin(plain) is get_origin(extra) is Callable
    assert get_args(plain) == ([Target, int], Target)
    assert get_args(extra) == ([Annotated[Target, "input"], int], Target)
    assert get_type_hints(fn) == {"a": Target, "return": type(None)}
    assert get_type_hints(fn, {"Target": int}, {"Target": str})["a"] is str
    assert fn.__annotations__["a"] == "Target"
    for expression in ("Missing", "list[Missing]"):
        error = error_for(expression)
        assert type(error) is NameError and error.name == "Missing"
        assert error.args == ("name 'Missing' is not defined",)
    # The bare name uses its original spelling; expression identifiers normalize.
    def unicode_names(bare: "K", nested: "list[K]"):
        pass
    assert get_type_hints(unicode_names, {"K": int, "K": bytes}) == {"bare": bytes, "nested": list[int]}
    spelling = "".join(["not_defined_", "K"])
    assert error_for(spelling).name is spelling
    assert type(error_for("Target.missing")) is AttributeError
    callback_error = error_for("callback()")
    assert type(callback_error) is NameError and callback_error.name == "callback_name"
    assert callback_error.args == ("user callback",)
    assert get_type_hints(fn, format=Format.FORWARDREF) == {"a": Target, "return": type(None)}

    class Deferred(Parent):
        missing: Unknown

    assert get_type_hints(Deferred, format=4)["missing"] == "Unknown"
    class_hints = get_type_hints(Deferred, format=Format.FORWARDREF)
    assert class_hints["item"] is Target and class_hints["missing"].__forward_arg__ == "Unknown"

    Deferred.__annotations__ = {"nested": "'Missing'", "arg": list["Missing"]}
    hints = get_type_hints(Deferred, format=Format.FORWARDREF)
    ref = hints["nested"]
    assert ref.__owner__ is Deferred and ref.__forward_is_class__ and not ref.__forward_is_argument__
    arg, = hints["arg"].__args__
    assert arg.__owner__ is None and arg.__forward_is_argument__ and not arg.__forward_is_class__

    for expr, fmt, expected in [(int, Format.VALUE, int), ("int", Format.VALUE, int),
                                ("Missing", Format.FORWARDREF, ForwardRef("Missing"))]:
        Deferred.__annotations__ = {"packed": tuple[*tuple[expr, ...]]}
        packed, = get_args(get_type_hints(Deferred, format=fmt)["packed"])
        assert get_origin(packed) is Unpack and get_args(packed) == (tuple[expected, ...],)

    def unresolved(arg: Unknown) -> None:
        pass

    hints = get_type_hints(unresolved, format=Format.FORWARDREF)
    assert hints["arg"].__owner__ is unresolved and hints["return"] is type(None)
    assert get_type_hints(unresolved, format=Format.STRING) == {"arg": "Unknown", "return": "None"}

    def register(content: Target, path):
        pass

    assert get_type_hints(register, format=3) == {"content": Target}
    marker = object()
    register.__annotations__ = {"content": list[Annotated[ReadOnly["Target"], marker]]}
    assert get_type_hints(register) == {"content": list[Target]}
    assert get_type_hints(register, include_extras=True) == {
        "content": list[Annotated[ReadOnly[Target], marker]]
    }
    register.__annotations__ = {"content": list["Undefined"]}
    assert get_type_hints(register, format=Format.FORWARDREF) == {"content": list[ForwardRef("Undefined")]}

    def provider(format):
        if format == Format.FORWARDREF:
            return {"content": Target}
        raise NameError("unavailable")

    register.__annotate__ = provider
    assert get_type_hints(register, format=Format.FORWARDREF) == {"content": Target}
    print("resolved type hints")


if __name__ == "__main__":
    current()
