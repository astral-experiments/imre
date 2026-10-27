"""Compare reported classes, exact types, and custom attribute access."""

import json

class Plain:
    declaring_module = __module__

assert Plain.__module__ == __name__ == Plain.declaring_module

def local_class():
    __name__ = "function local"
    class Local:
        pass
    return Local

assert local_class().__module__ == __name__

class Explicit:
    __module__ = "selected.module"

assert Explicit.__module__ == "selected.module"

class Removed:
    del __module__

assert Removed.__module__ == __name__

class Advertised:
    @property
    def __class__(self):
        return str

class Data:
    __class__ = 42

class Fallback:
    def __getattr__(self, name):
        return "fallback " + name

class Key:
    @property
    def __class__(self):
        raise AttributeError("missing")
    def __getattr__(self, name):
        return "field " + name

class Loud:
    @property
    def __class__(self):
        raise LookupError("called")

class Guarded:
    def __getattribute__(self, name):
        if name == "__class__":
            return str
        return object.__getattribute__(self, name)
    tag = "stored"

class GuardedChild(Guarded):
    pass


class Missing:
    def __getattribute__(self, name):
        if name == "__class__":
            raise AttributeError("hidden")
        return super().__getattribute__(name)
    def __getattr__(self, name):
        return "fallback " + name
    constant = 7


class Ordinary:
    __getattribute__ = object.__getattribute__


def current():
    for obj in (Plain(), Advertised(), Data(), Fallback(), Key(), Guarded()):
        cls = obj.__class__
        print(type(obj).__name__, cls is str, cls is type(obj), 42 == cls, cls == "field __class__")
    print(Plain.__class__ is type, Advertised.__class__ is type, Data.__class__ is type)
    print(Guarded().tag, object.__getattribute__(Guarded(), "__class__") is Guarded)
    data = Data()
    data.__class__ = "shadow"
    print(data.__class__, type(data) is Data)
    try:
        Loud().__class__
    except LookupError as exc:
        print(exc.args)
    for obj in (None,True,1,"x",b"x",[],{},(),set(),int,dict,len,json):
        print(obj.__class__ is type(obj))
    wrapped = Missing()
    print(wrapped.__class__, wrapped.constant, wrapped.empty)
    print(GuardedChild().__class__ is str, type(GuardedChild()) is GuardedChild)
    print(Ordinary().__class__ is Ordinary, Plain().__getattribute__("__class__") is Plain)
    for call in (
        lambda: object.__getattribute__(),
        lambda: object.__getattribute__(1),
        lambda: object.__getattribute__(1, "a", "b"),
        lambda: object.__getattribute__(1, 3),
        lambda: object.__getattribute__(1, name="a"),
        lambda: object.__getattribute__(1, "foo"),
        lambda: getattr(Plain(), 1),
        lambda: hasattr(Plain(), 1),
    ):
        try:
            call()
        except (TypeError, AttributeError) as exc:
            print(exc.args)
