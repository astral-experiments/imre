import types

marker = 11


def original(x, *, y=2):
    return marker + x + y


def documented():
    """original documentation"""
    return 9


def current():
    copied = types.FunctionType(original.__code__, original.__globals__, "copied", (4,), kwdefaults={"y": 3})
    print(copied(), copied.__name__, copied.__qualname__, copied.__defaults__, copied.__kwdefaults__)
    print(copied.__globals__ is original.__globals__)
    namespace = {"marker": 7}
    alternate = types.FunctionType(code=original.__code__, globals=namespace, argdefs=(1,), kwdefaults={"y": 5})
    print(alternate(), alternate.__globals__ is namespace)
    namespace["marker"] = 20
    print(alternate())
    for code, globals, defaults in ((0, {}, None), (original.__code__, 0, None), (original.__code__, {}, 3)):
        try:
            types.FunctionType(code, globals, argdefs=defaults)
        except TypeError as error:
            print(error.args)
        else:
            print("unexpected success")

    class Name(str):
        pass

    name = Name("copy")
    copied_name = types.FunctionType(documented.__code__, {}, name)
    print(copied_name.__name__ is name, copied_name())
    for invoke in (
        lambda: types.FunctionType(),
        lambda: types.FunctionType(original.__code__),
        lambda: types.FunctionType(original.__code__, {}, 7),
        lambda: types.FunctionType(original.__code__, {}, closure=1),
        lambda: types.FunctionType(original.__code__, {}, kwdefaults=1),
        lambda: types.FunctionType(original.__code__, {}, closure=(1,)),
    ):
        try:
            invoke()
        except (TypeError, ValueError) as error:
            print(type(error).__name__, error.args)

    documented.__doc__ = "changed"
    copied_doc = types.FunctionType(documented.__code__, {})
    print(copied_doc.__doc__, copied_doc())
    namespace = {}
    exec("def source_function():\n    'source documentation'\n    return 5\n", namespace)
    source_copy = types.FunctionType(namespace["source_function"].__code__, namespace)
    print(source_copy.__doc__, source_copy())
