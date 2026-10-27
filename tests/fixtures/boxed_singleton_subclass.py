import types


def current():
    for cls, singleton in (
        (types.NoneType, None),
        (types.NotImplementedType, NotImplemented),
        (types.EllipsisType, ...),
    ):
        print(cls() is singleton)
        try:
            class Child(cls):
                pass
        except TypeError as error:
            print(error.args)
        else:
            print('unexpected subclass')
        try:
            class Child(object, cls):
                pass
        except TypeError as error:
            print(error.args)
        else:
            print('unexpected subclass')

    class Base:
        pass

    class Child(Base):
        pass

    print(isinstance(Child(), Base))
