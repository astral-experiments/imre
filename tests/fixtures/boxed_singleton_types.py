import types


def current():
    for cls, singleton in (
        (types.NoneType, None),
        (types.NotImplementedType, NotImplemented),
        (types.EllipsisType, ...),
    ):
        print(cls() is singleton, cls() is cls())
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
