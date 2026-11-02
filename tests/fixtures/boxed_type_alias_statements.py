from typing import TypeAliasType


type Forward = later
type Recursive = tuple[Forward, list[Recursive]]


def current():
    value = []
    eager = TypeAliasType("Eager", value)
    assert type(eager) is TypeAliasType and eager.__value__ is value
    assert eager.__module__ == __name__ and eager.__name__ == "Eager"
    assert eager.__type_params__ == eager.__parameters__ == ()
    assert eager.evaluate_value(1) is value and eager.__reduce__() == "Eager"
    assert TypeAliasType(name="N", value=None, type_params=()).__value__ is None

    assert type(Forward) is TypeAliasType
    assert Forward.__module__ == __name__ and Forward.__name__ == "Forward"
    assert Forward.__type_params__ == Forward.__parameters__ == ()
    assert repr(Forward) == "Forward"
    try:
        Forward.__value__
    except NameError as exc:
        assert exc.name == "later"
    else:
        raise AssertionError("evaluated an unbound name")

    global later
    later = int
    assert Forward.__value__ is int
    later = str
    assert Forward.__value__ is int and Forward.evaluate_value() is str
    assert Forward.evaluate_value is Forward.evaluate_value
    first, nested = Recursive.__value__.__args__
    assert first is Forward and nested.__args__[0] is Recursive

    scope = {"__name__": "other", "value": None}
    exec(compile("type Alias = value", "late.py", "exec"), scope)
    alias = scope["Alias"]
    assert type(alias) is TypeAliasType and alias.__module__ == "other"
    assert alias.__value__ is None
    scope["value"] = bytes
    assert alias.__value__ is None and alias.evaluate_value() is bytes
    try:
        alias.__value__ = int
    except AttributeError:
        pass
    else:
        raise AssertionError("writable alias value")
    try:
        compile("if False:\n    type Invalid = (x := 1)", "alias.py", "exec")
    except SyntaxError as exc:
        assert exc.filename == "alias.py" and exc.lineno == 2
    else:
        raise AssertionError("accepted a named expression in an alias")
    def make_alias():
        later = int
        type Pair[K, V] = tuple[V, K, later]
        type Local = Pair
        later = bytes
        return Local.__value__

    pair = make_alias()
    k, v = pair.__type_params__
    assert pair.__parameters__ is pair.__type_params__
    assert pair.__value__.__args__ == (v, k, bytes)
    assert pair.__value__[str, int] == tuple[str, int, bytes]
    assert pair[int].__origin__ is pair and pair[int].__args__ == (int,)
    assert list[pair].__parameters__ == (k, v)
    assert make_alias().__type_params__[0] is not k
    # Separate exec locals receive the declaration; the evaluator sees parameters and globals.
    namespace = {"__name__": "other", "Key": str, "late": int}
    bindings = {"late": float}
    code = compile("type Pair[Key, Value] = tuple[Value, Key, late]", "generic.py", "exec")
    exec(code, namespace, bindings)
    first = bindings["Pair"]
    key, value = first.__type_params__
    assert first.__module__ == "other" and first.__parameters__ is first.__type_params__
    assert "Pair" not in namespace and "Key" not in bindings and namespace["Key"] is str
    namespace["late"] = bytes
    assert first.__value__.__args__ == (value, key, bytes)
    namespace["late"] = bool
    assert first.__value__.__args__[2] is bytes and first.evaluate_value().__args__[2] is bool
    exec(code, namespace, bindings)
    assert bindings["Pair"].__type_params__[0] is not key
    print("lazy type aliases")
