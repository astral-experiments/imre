from enum import Enum, IntEnum, IntFlag

class Descriptor:
    def __get__(self, obj, owner):
        return ("descriptor", obj is None, owner.__name__)

class SetterOnly:
    def __set__(self, obj, value):
        pass

def current():
    class C(Enum):
        A = 1
        B = 1
        desc = Descriptor()
        setter = SetterOnly()
        @classmethod
        def cls(cls):
            return cls
        @staticmethod
        def static(value):
            return value
        @property
        def name2(self):
            return self.name
    print(tuple(C.__members__), C.A is C.B, C(1) is C.A)
    print(C.cls() is C, C.A.cls() is C)
    print(C.desc, C.A.desc, C.A.name2, C.static(17), C.A.static(19))
    print(type(C.setter).__name__, type(C.A.setter).__name__)

    member = C.A
    member.label = "active"
    member._private = None
    directory = dir(member)
    assert {"name", "value", "label", "desc", "setter", "cls", "name2"} <= set(directory)
    assert {"A", "B", "_private"}.isdisjoint(directory)

    class Labels(Enum):
        @property
        def label(self):
            raise AssertionError("dir called a property getter")

    class Labeled(Labels):
        label = 1
        OTHER = 2

    assert "label" in dir(Labeled.OTHER)

    class Codes(IntEnum):
        name = 1
        value = 2
        alias = 2

    assert {"name", "value", "alias", "bit_length"} <= set(dir(Codes.alias))
    assert Codes.alias.value == 2 and Codes.alias.name == "value"

    class Explicit(Enum):
        ITEM = 1
        def __dir__(self):
            return ["z", "a"]

    assert dir(Explicit.ITEM) == ["a", "z"]

    # CPython test_enum.TestHelpers.test_is_descriptor sets each name on an instance.
    class Plain:
        pass

    for name in ("__get__", "__set__", "__delete__"):
        marker = Plain()
        setattr(marker, name, 1)

        class Fields(Enum):
            regular = 1
            descriptor = marker

        print(tuple(Fields.__members__), Fields.descriptor is marker)

    class Interrupted:
        def __getattr__(self, name):
            if name == "__set__":
                raise ValueError("descriptor lookup")
            raise AttributeError(name)

    try:
        class Broken(Enum):
            entry = Interrupted()
    except ValueError as error:
        print(error.args)

    # Integer enums inherit int's numeric string slot independently of their repr.
    class Status(IntEnum):
        OK = 1
        def __repr__(self):
            return "status"

    class Bits(IntFlag):
        READ = 1
        WRITE = 2
        def __repr__(self):
            return "bits"

    for member in (C.B, Status.OK, Bits(3)):
        factory, arguments = member.__reduce_ex__(proto=4)
        assert factory is type(member) and arguments == (member.value,)
        assert factory(*arguments) is member

    assert IntEnum.__str__ is IntFlag.__str__ is int.__repr__
    assert (repr(Status.OK), str(Status.OK)) == ("status", "1")
    assert (repr(Bits(3)), str(Bits(3))) == ("bits", "3")
    assert Bits.__bool__ is int.__bool__
    assert not (Bits.READ & Bits.WRITE) and bool(Bits.READ | Bits.WRITE)
    Bits.READ._value_ = 0
    assert bool(Bits.READ) and int(Bits.READ) == 1

    class Plain(Enum):
        ZERO = 0

    class Empty:
        def __len__(self):
            return 0

    class Number(int):
        def __len__(self):
            raise AssertionError("int.__bool__ must precede __len__")

    class Override(Number):
        def __bool__(self):
            return True

    assert Plain.ZERO and not Empty()
    assert not Number(0) and Number(-1) and Number(1 << 100)
    assert Override(0) and not int.__bool__(Override(0))

    Status.__str__ = lambda self: self.name
    assert str(Status.OK) == "OK" and repr(Status.OK) == "status"

    # RegexFlag explicitly chooses object's delegation to its own repr.
    import re
    assert re.RegexFlag.__str__ is object.__str__
    assert str(re.RegexFlag(2)) == repr(re.RegexFlag(2))

    # A tuple mixin's generic alias must not mask an enum name or a metaclass slot.
    from collections import namedtuple
    from types import GenericAlias

    class Pair(namedtuple("Pair", "x y"), Enum):
        ORIGIN = (0, 0)

    assert Pair["ORIGIN"] is Pair.ORIGIN
    try:
        Pair["missing"]
    except KeyError as exc:
        assert exc.args == ("missing",)
    else:
        raise AssertionError("enum lookup accepted a missing member")

    class Meta(type):
        def __getitem__(cls, key):
            return ("metaclass", key)

    class Specialized(tuple, metaclass=Meta):
        pass

    assert Specialized[int] == ("metaclass", int)
    assert type(tuple[int]) is GenericAlias and tuple[int].__args__ == (int,)
