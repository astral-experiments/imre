import typing as t


class Named(t.Protocol):
    def name(self): ...


@t.runtime_checkable
class HasName(Named, t.Protocol):
    pass


class Item:
    def name(self):
        return "item"


def local():
    class Local(t.Protocol):
        pass

    return Local._is_protocol


print(local(), isinstance(Item(), HasName), isinstance(object(), HasName))
try:
    t.runtime_checkable(Item)
except TypeError as exc:
    print(type(exc).__name__)
