import types


def current():
    for cls, singleton in (
        (types.NoneType, None),
        (types.NotImplementedType, NotImplemented),
        (types.EllipsisType, ...),
    ):
        print(cls() is singleton, cls() is cls())
        new = cls.__dict__["__new__"]
        assert new.__self__ is cls
        assert new(cls) is singleton
        try:
            new(object)
        except TypeError as error:
            print(error.args)
        else:
            print("unexpected success")
        for positional in (True, False):
            try:
                if positional:
                    cls(1)
                else:
                    cls(value=1)
            except TypeError as error:
                print(error.args)
            else:
                print("unexpected success")
