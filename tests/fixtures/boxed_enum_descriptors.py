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

    assert IntEnum.__str__ is IntFlag.__str__ is int.__repr__
    assert (repr(Status.OK), str(Status.OK)) == ("status", "1")
    assert (repr(Bits(3)), str(Bits(3))) == ("bits", "3")

    Status.__str__ = lambda self: self.name
    assert str(Status.OK) == "OK" and repr(Status.OK) == "status"

    # RegexFlag explicitly chooses object's delegation to its own repr.
    import re
    assert re.RegexFlag.__str__ is object.__str__
    assert str(re.RegexFlag(2)) == repr(re.RegexFlag(2))
