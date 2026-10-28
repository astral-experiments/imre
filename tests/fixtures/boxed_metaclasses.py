def current():
    from enum import Enum

    class Policy(Enum):
        DEFAULT = "default"
        REQUIRED = "required"

    assert Policy("required") is Policy.REQUIRED and Policy["DEFAULT"] is Policy.DEFAULT
    assert [member.value for member in Policy] == ["default", "required"]

    class ParentMeta(type):
        pass

    class DerivedMeta(ParentMeta):
        def __new__(meta, name, bases, namespace, *, label="default"):
            namespace["label"] = label
            return type.__new__(meta, name, bases, namespace)

        def __init__(cls, name, bases, namespace, *, label="default"):
            cls.initialized = (name, label)

    class Left(metaclass=ParentMeta):
        pass

    class Right(metaclass=DerivedMeta, label="right"):
        pass

    class Combined(Left, Right, metaclass=ParentMeta):
        pass

    class Inherited(Combined):
        pass

    assert type(Left) is ParentMeta and type(ParentMeta) is type
    assert type(Right) is type(Combined) is type(Inherited) is DerivedMeta
    assert Combined.__bases__ == (Left, Right) and Inherited.__bases__ == (Combined,)
    assert Right.label == "right" and Right.initialized == ("Right", "right")
    assert Combined.label == "default" and Inherited.initialized == ("Inherited", "default")
    print("selection and construction")

    class SlottedDoc(metaclass=DerivedMeta):
        __slots__ = ("__doc__",)

    slotted = SlottedDoc()
    assert not hasattr(slotted, "__doc__")
    slotted.__doc__ = "instance documentation"
    assert slotted.__doc__ == "instance documentation"
    del slotted.__doc__
    assert not hasattr(slotted, "__doc__")

    class OtherMeta(type):
        pass

    class Other(metaclass=OtherMeta):
        pass

    try:
        class Conflict(Right, Other):
            pass
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("incompatible metaclasses were accepted")

    class Predicates(type):
        def __instancecheck__(cls, obj):
            raise ValueError("instance hook")

        def __subclasscheck__(cls, child):
            raise LookupError("subclass hook")

    class Checked(metaclass=Predicates):
        pass

    assert isinstance(Checked(), Checked)
    try:
        isinstance(3, Checked)
    except ValueError as error:
        print(str(error))
    else:
        raise AssertionError("instance hook exception was lost")
    try:
        issubclass(Checked, Checked)
    except LookupError as error:
        print(str(error))
    else:
        raise AssertionError("subclass hook exception was lost")

    class Ordinary:
        pass

    copied = {"attribute": 42}
    made = type.__new__(ParentMeta, "Made", (Ordinary,), copied)
    copied["attribute"] = 0
    assert type(made) is ParentMeta and made.__bases__ == (Ordinary,) and made.attribute == 42
    assert made.__module__ == __name__
    namespace = globals()
    original_name = namespace["__name__"]
    try:
        for value in ("renamed.module", None):
            namespace["__name__"] = value
            changed = type.__new__(ParentMeta, "Changed", (), {})
            assert changed.__module__ == value
    finally:
        namespace["__name__"] = original_name
    named = type.__new__(ParentMeta, "Named", (), {"__module__": "another.module"})
    assert named.__module__ == "another.module"
    assert type(Ordinary()) is Ordinary and type(Ordinary) is type
    print("ordinary class and independent type dictionary")

    class Extra:
        marker = 1

    class Ordered(type):
        def mro(cls):
            assert cls.__mro__ is None
            return (cls, Extra, object)

    class WithOrder(metaclass=Ordered):
        pass

    assert WithOrder.__bases__ == (object,) and WithOrder.__mro__ == (WithOrder, Extra, object)
    assert type.mro(WithOrder) == [WithOrder, object] and int.mro() == [int, object]
    Extra.marker = 2
    assert WithOrder.marker == 2

    writes = []
    namespaces = []
    class Namespace(dict):
        def __setitem__(self, key, value):
            writes.append(key)
            dict.__setitem__(self, key, value)

    class Prepare(type):
        @staticmethod
        def __prepare__(name, bases, **keywords):
            namespace = Namespace()
            namespaces.append(namespace)
            return namespace

    class Tracked(metaclass=Prepare):
        def assign(self):
            self.z = self.a = 1

    assert writes == ["__module__", "__qualname__", "__firstlineno__", "assign", "__static_attributes__", "__classdictcell__"]
    assert Tracked.__static_attributes__ == ("a", "z") and Tracked.__firstlineno__ > 0
    assert Tracked.__annotations__ == {}
    class_dict = namespaces[0]["__classdictcell__"].cell_contents
    assert type(class_dict) is dict and class_dict is not namespaces[0]
    Tracked.marker = 42
    assert class_dict["marker"] == 42 and "__classdictcell__" not in class_dict

    class Child(Tracked):
        def declaring(self):
            return __class__

    assert namespaces[1]["__classcell__"].cell_contents is Child and Child().declaring() is Child
    for key in ("__classcell__", "__classdictcell__"):
        try:
            type("InvalidCell", (), {key: 1})
        except TypeError as error:
            assert str(error) == key + " must be a nonlocal cell, not <class 'int'>"
        else:
            raise AssertionError("invalid cell accepted")

    class Base:
        def __init_subclass__(cls, *, label, **kw):
            cls.label = label
            super().__init_subclass__(**kw)

    class Labeled(Base, metaclass=Prepare, label="custom"):
        pass

    class OrdinaryLabeled(Base, label="ordinary"):
        pass

    assert Labeled.label == "custom" and OrdinaryLabeled.label == "ordinary"
    print("metaclass MRO and prepared namespace")

    class NamespaceMapping:
        def __init__(self):
            self.items = {}
        def __getitem__(self, name):
            if name == "injected":
                return 41
            if name == "fault":
                raise LookupError("class body lookup")
            return self.items[name]
        def __setitem__(self, name, value):
            self.items[name] = value
        def __delitem__(self, name):
            del self.items[name]

    def namespace_meta(name, bases, namespace):
        return namespace

    def prepare_mapping(name, bases):
        return NamespaceMapping()

    namespace_meta.__prepare__ = prepare_mapping
    class Mapped(metaclass=namespace_meta):
        value = injected + 1
        saved = value
        del value
    assert Mapped["saved"] == 42 and "value" not in Mapped.items
    try:
        class Fault(metaclass=namespace_meta):
            value = fault
    except LookupError as exc:
        assert str(exc) == "class body lookup"
    else:
        raise AssertionError("prepared mapping exception was lost")

    class ShortMRO(type):
        def mro(cls):
            return [cls]

    class Short(metaclass=ShortMRO):
        pass
    assert Short.__mro__ == (Short,)
    try:
        class ExtraKeyword(metaclass=ShortMRO, extra=True):
            pass
    except TypeError as exc:
        assert str(exc) == "super.__init_subclass__() takes no keyword arguments"
    else:
        raise AssertionError("extra class keyword accepted")

    class Owner:
        class __Private(metaclass=Prepare):
            def write(self):
                self.__secret = 42
        child = __Private

    assert Owner.child.__name__ == "__Private"
    assert Owner.child.__static_attributes__ == ("__secret",)
    obj = Owner.child()
    obj.write()
    assert obj._Private__secret == 42

    from enum import IntEnum, _simple_enum

    @_simple_enum(IntEnum)
    class Status:
        NOT_FOUND = 404

    assert Status.__bases__ == (IntEnum,) and Status.__mro__[1] is IntEnum
    assert {Status.NOT_FOUND: "missing"}[404] == "missing"
