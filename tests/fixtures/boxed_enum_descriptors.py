from enum import Enum

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
