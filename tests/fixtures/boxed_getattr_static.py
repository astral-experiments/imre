from inspect import getattr_static
from types import SimpleNamespace


class ReadOnly:
    def __get__(self, instance, owner):
        raise AssertionError("descriptor was called")


class Data(ReadOnly):
    def __delete__(self, instance):
        raise AssertionError("descriptor was called")


class Meta(type):
    inherited = ReadOnly()


class Base(metaclass=Meta):
    data = Data()
    nondata = ReadOnly()

    def __getattribute__(self, name):
        raise AssertionError("attribute hook was called")

    def __getattr__(self, name):
        raise AssertionError("attribute hook was called")


class Child(Base):
    pass


class Hidden:
    value = "class"

    @property
    def __dict__(self):
        raise AssertionError("shadowed dictionary was read")


item = Child()
item.nondata = "instance"
item.only_here = None
object.__getattribute__(item, "__dict__")["data"] = 1
assert getattr_static(item, "data") is vars(Base)["data"]
assert getattr_static(obj=item, attr="nondata") == "instance"
assert getattr_static(item, "only_here") is None
assert getattr_static(item, "__qualname__", item) is item
assert getattr_static(Child, "nondata") is vars(Base)["nondata"]
assert getattr_static(Child, "inherited") is vars(Meta)["inherited"]
assert getattr_static(item, "missing", default=item) is item
try:
    getattr_static(item, "missing")
except AttributeError as error:
    assert error.args == ("missing",)
else:
    raise AssertionError("missing attribute was found")

hidden = Hidden()
hidden.value = "instance"
assert getattr_static(hidden, "value") == "class"
assert getattr_static(SimpleNamespace(item=42), "item") == 42
assert getattr_static("example", "lower") is str.lower
assert getattr_static(Base, "__repr__") is object.__repr__
assert getattr_static(object, "__str__") is object.__str__
print("static attributes")
