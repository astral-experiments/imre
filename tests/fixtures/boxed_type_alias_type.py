from typing import TypeAliasType, TypeVar


def current():
    eager = TypeAliasType("Eager", None)
    assert type(eager) is TypeAliasType and eager.__value__ is None
    assert eager.__name__ == "Eager" and eager.__module__ == __name__
    assert eager.evaluate_value(1) is None
    parameter = TypeVar("P")
    generic = TypeAliasType("Generic", list[parameter], type_params=(parameter,))
    assert generic.__type_params__ == (parameter,) and generic[int].__origin__ is generic

    events = []
    def resolve():
        events.append(1)
        return result
    type Later = resolve()
    try:
        Later.__value__
    except NameError:
        pass
    else:
        raise AssertionError("the unresolved name was evaluated successfully")
    result = int
    assert Later.__value__ is int
    result = str
    assert Later.__value__ is int and events == [1, 1]
    assert Later.evaluate_value(1) is str and events == [1, 1, 1]

    class Owner:
        field = int
        type Alias = field
        type __Private = field

    assert Owner._Owner__Private.__name__ == "__Private"
    assert Owner._Owner__Private.evaluate_value.__qualname__.endswith("Owner.__Private")
    Owner.field = bytes
    assert Owner.Alias.__value__ is bytes
    type Recursive = list[Recursive]
    assert Recursive.__value__.__args__ == (Recursive,)
    assert (Recursive | int).__args__ == (Recursive, int)
    store = {}
    exec("type Alias = Later\nLater = int", store)
    assert store["Alias"].__value__ is int and store["Alias"].__module__ is None


if __name__ == "__main__":
    current()
