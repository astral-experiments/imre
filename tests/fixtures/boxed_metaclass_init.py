"""Methods added by an inherited metaclass retain their defining defaults."""

def current():
    calls = []

    class Factory(type):
        def __init__(cls, name, bases, namespace):
            calls.append((cls.__name__, tuple(base.__name__ for base in bases)))
            for label in namespace.get("labels", ()):
                def generated(self, label=label):
                    return self.__class__.__name__, label
                generated.__name__ = "test_" + label
                setattr(cls, generated.__name__, generated)
            namespace["unpublished"] = True

    namespace = {"labels": ("one", "two")}
    Base = Factory("Base", (), namespace)

    class Child(Base):
        labels = ("three",)

    Generated = type("Generated", (Base,), {"labels": ("four",)})
    print(type(Base) is Factory, isinstance(Child, Factory), type(Child) is Factory,
          type(Generated) is Factory)
    print(calls, namespace["unpublished"], hasattr(Child, "unpublished"))
    for cls in (Base, Child, Generated):
        obj = cls()
        for name in sorted(key for key in dir(cls) if key.startswith("test_")):
            method = getattr(obj, name)
            print(name, method.__name__, method())

    for args in [(7, (), {}), ("MissingTuple", [], {}), ("MissingDict", (), [])]:
        try:
            Factory(*args)
        except TypeError as error:
            print(str(error))

    class BrokenFactory(type):
        def __init__(cls, name, bases, namespace):
            if bases:
                raise ValueError(name)

    OtherBase = BrokenFactory("OtherBase", (), {})
    try:
        class Broken(OtherBase):
            pass
    except ValueError as error:
        print(error.args)
    try:
        class Incompatible(Base, OtherBase):
            print("class body must not be reached")
    except TypeError as error:
        print(str(error))
