"""Compare reported classes, exact types, and custom attribute access."""

import json

for cls, bases, mro in (
    (object, (), (object,)),
    (dict, (object,), (dict, object)),
    (bool, (int,), (bool, int, object)),
    (KeyError, (LookupError,), (KeyError, LookupError, Exception, BaseException, object)),
):
    assert cls.__bases__ == bases and cls.__mro__ == mro
    assert cls.__bases__ is cls.__bases__ and cls.__mro__ is cls.__mro__


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


def private_names():
    _Owner__value = "outer"
    __value = "unrelated"

    class Owner:
        __member = "class member"

        def __write(self, __argument):
            nonlocal __value
            __value = __argument
            return self.__member

        def members(self, __argument):
            yield self.__member
            yield __argument

        def update(self):
            return self.__write("updated")

    print(Owner().update(), _Owner__value, __value)
    print(list(Owner().members("argument")))
    print(Owner._Owner__write.__name__, hasattr(Owner, "__member"))


def class_patterns():
    class Record:
        values: list[int]

    class Other:
        values: list[int]

    record = Record()
    record.values = [7]
    alias = Record
    match record:
        case Other(values=_):
            raise AssertionError("matched an unrelated class")
        case alias(values=[first]) as whole:
            assert first == 7 and whole is record
        case _:
            raise AssertionError("missed the declared field")


def ordinary_slot_members():
    class Base:
        __slots__ = ("field", "__weakref__")

    class Child(Base):
        __slots__ = ("field", "__dict__")

    obj = Child()
    root = Base.__dict__["field"]
    child = Child.__dict__["field"]
    root.__set__(obj, 1)
    child.__set__(obj, 2)
    obj.__dict__["field"] = 3
    assert obj.field == 2 and root.__get__(obj, Child) == 1
    del obj.field
    try:
        obj.field
    except AttributeError:
        pass
    else:
        raise AssertionError("uninitialized slot fell through to instance dictionary")
    assert obj.__dict__["field"] == 3 and root.__get__(obj, Child) == 1


def reported_instance_checks():
    class Child(Loud):
        pass

    class Hidden:
        @property
        def __class__(self):
            raise AttributeError("missing")

    class Reject(type):
        def __instancecheck__(cls, obj):
            return False

    class Reported(metaclass=Reject):
        pass

    class Proxy:
        __class__ = Reported

    assert isinstance(Child(), Loud)
    assert isinstance(Advertised(), str)
    assert isinstance(Guarded(), str)
    assert not isinstance(Data(), str)
    assert not isinstance(Hidden(), str)
    assert not isinstance(Proxy(), Reported)
    try:
        isinstance(Loud(), str)
    except LookupError as error:
        assert error.args == ("called",)
    else:
        raise AssertionError("isinstance suppressed the reported class error")


def class_reassignment():
    import gc
    import weakref

    class Owner:
        pass

    class Reader:
        def read(self):
            return self.left, self.right

    class Upgraded(Reader, Owner):
        pass

    class Slotted:
        __slots__ = ("left",)

    class InheritedSlot(Slotted):
        pass

    prepared = Upgraded()
    prepared.right, prepared.left = 3, 4
    for materialize in (False, True):
        owner = Owner()
        owner.left, owner.right = 1, 2
        namespace = owner.__dict__ if materialize else None
        ref = weakref.ref(owner)
        owner.__class__ = Upgraded
        assert type(owner) is owner.__class__ is Upgraded and ref() is owner
        assert owner.read() == (1, 2) and prepared.read() == (4, 3)
        if materialize:
            assert owner.__dict__ is namespace
        assert owner.__dict__ == {"left": 1, "right": 2}

        for invalid in (17, int, Slotted, InheritedSlot):
            try:
                owner.__class__ = invalid
            except TypeError:
                pass
            else:
                raise AssertionError("accepted an incompatible class")
            assert type(owner) is Upgraded and owner.read() == (1, 2)

        object.__dict__["__class__"].__set__(owner, Owner)
        assert type(owner) is Owner and ref() is owner and not hasattr(owner, "read")

    class Managed:
        @property
        def __class__(self):
            return self.reported

        @__class__.setter
        def __class__(self, value):
            self.reported = value

    managed = Managed()
    managed.__class__ = Upgraded
    assert type(managed) is Managed and managed.__class__ is Upgraded

    class ClassSlot:
        __slots__ = ("__class__",)

    slotted = ClassSlot()
    slotted.__class__ = 42
    assert type(slotted) is ClassSlot and slotted.__class__ == 42

    finalized = []

    class Notifying(Owner):
        def __del__(self):
            finalized.append(self.label)

    def release(initial, replacement, label):
        obj = initial()
        obj.label = label
        obj.__class__ = replacement
        return weakref.ref(obj)

    added = release(Owner, Notifying, "added")
    removed = release(Notifying, Owner, "removed")
    gc.collect()
    assert added() is None and removed() is None and finalized == ["added"]


def current():
    class_reassignment()
    ordinary_slot_members()
    class_patterns()
    reported_instance_checks()

    class Journal:
        pass

    class Name(str):
        pass

    original = Journal.__name__
    assert original is Journal.__name__
    replacement = Name("Journáux")
    Journal.__name__ = replacement
    assert Journal.__name__ is replacement
    assert "__name__" not in Journal.__dict__
    assert Journal.__qualname__.endswith("Journal")
    try:
        del Journal.__name__
    except TypeError as exc:
        assert "immutable type 'Journáux'" in str(exc)
    else:
        raise AssertionError("a class name cannot be deleted")
    try:
        Journal.__name__ = "nul\0name"
    except ValueError:
        assert Journal.__name__ is replacement
    else:
        raise AssertionError("a class name cannot contain a null character")

    private_names()
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
