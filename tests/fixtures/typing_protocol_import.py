import typing as t


class Named(t.Protocol):
    def name(self): ...


@t.runtime_checkable
class HasName(Named, t.Protocol):
    pass


class Item:
    def name(self):
        return "item"


print(isinstance(Item(), HasName), isinstance(object(), HasName))
try:
    t.runtime_checkable(Item)
except TypeError as exc:
    print(type(exc).__name__)
