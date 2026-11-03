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
class_field = getattr_static(item, "__class__")
assert class_field is getattr_static(Child, "__class__")
assert class_field.__objclass__ is object
assert class_field.__get__(item) is Child
assert class_field.__get__(None, Child) is class_field

dictionary = vars(Base)["__dict__"]
assert getattr_static(item, "__dict__") is dictionary
assert getattr_static(Child, "__dict__") is dictionary
assert dictionary.__objclass__ is Base
assert dictionary.__get__(item) is object.__getattribute__(item, "__dict__")
dictionary.__set__(item, {"replaced": 2})
assert getattr_static(item, "replaced") == 2
dictionary.__delete__(item)
assert getattr_static(item, "replaced", item) is item
try:
    dictionary.__get__(Hidden())
except TypeError:
    pass
else:
    raise AssertionError("dictionary descriptor accepted an unrelated receiver")
class Mixin:
    pass


class ErrorWithMixin(Mixin, Exception):
    pass


exc = ErrorWithMixin()
exc.extra = 47
inherited_dict = vars(Mixin)["__dict__"]
assert inherited_dict.__get__(exc) is vars(exc)
assert vars(exc)["extra"] == 47
replacement = {"extra": 5}
inherited_dict.__set__(exc, replacement)
assert vars(exc) is replacement
try:
    inherited_dict.__delete__(exc)
except TypeError as error:
    assert error.args == ("cannot delete __dict__",)
else:
    raise AssertionError("exception dictionary was deleted")
class NamespaceWithMixin(Mixin, SimpleNamespace):
    pass


ns = NamespaceWithMixin(answer=42)
assert inherited_dict.__get__(ns) is vars(ns)
try:
    inherited_dict.__set__(ns, {})
except AttributeError as error:
    assert error.args == ("readonly attribute",)
else:
    raise AssertionError("namespace dictionary was replaced")
namespace = vars(object)
assert type(namespace).__name__ == "mappingproxy"
assert "__dict__" not in namespace
assert object.__class__ is type and Child.__class__ is type(Child)
assert namespace["__class__"] is class_field
assert namespace["__delattr__"] is object.__delattr__ is Child.__delattr__
class Ordinary:
    pass


assert Ordinary.__class__ is type
ordinary = Ordinary()
ordinary.marker = 42
bound = ordinary.__delattr__
assert type(bound).__name__ == "method-wrapper" and bound.__self__ is ordinary
bound("marker")
assert not hasattr(ordinary, "marker")
assert namespace["__eq__"](item, item) is True
assert namespace["__eq__"](item, Child()) is NotImplemented
assert namespace["__ge__"](item, item) is NotImplemented
hook = namespace["__subclasshook__"]
assert type(hook).__name__ == "classmethod_descriptor"
assert hook.__get__(None, Child).__self__ is Child
assert Child.__subclasshook__(Base) is NotImplemented
assert object.__dir__(object()) == list(namespace)
ordinary.z_last = 1
ordinary.a_first = 2
directory = namespace["__dir__"](ordinary)
expected = list(dict.fromkeys([*vars(ordinary), *vars(Ordinary), *namespace]))
assert type(directory) is list and directory == expected
assert dir(ordinary) == sorted(expected)
assert ordinary.__dir__() == directory and ordinary.__dir__() is not directory
try:
    namespace["missing"] = 3
except TypeError:
    pass
else:
    raise AssertionError("object namespace was writable")
print("static attributes")
