from annotationlib import Format, ForwardRef
from typing import Annotated, ReadOnly, get_type_hints


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
    assert get_type_hints(Parent) == {"item": Target, "values": list[Target]}
    assert get_type_hints(Child) == {"item": bytes, "values": list[Target]}
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
