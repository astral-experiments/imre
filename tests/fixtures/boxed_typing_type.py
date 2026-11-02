from typing import Generic, Type, TypeVar, get_args, get_origin


def current():
    class User: ...

    assert repr(Type) == "typing.Type"
    assert get_origin(Type) is type and get_args(Type) == ()
    assert Type.__mro_entries__((Type,)) == (type, Generic)
    assert isinstance(User, Type) and not isinstance(User(), Type)
    assert issubclass(type, Type) and not issubclass(User, Type)
    assert Type[User] is Type[User]
    assert get_origin(Type[User]) is type and get_args(Type[User]) == (User,)
    assert get_args(Type[None]) == (type(None),)
    parameter = TypeVar("parameter")
    assert Type[parameter].__parameters__ == (parameter,)
    assert Type[parameter][User] == Type[User]

    for alias in (Type, Type[int]):
        try:
            alias(1)
        except TypeError as error:
            assert str(error) == "Type Type cannot be instantiated; use type() instead"
        else:
            raise AssertionError("instantiated typing.Type")

    for args in ((), (str, int)):
        try:
            Type[args]
        except TypeError as error:
            assert f"actual {len(args)}, expected 1" in str(error)
        else:
            raise AssertionError("accepted the wrong number of type arguments")

    for check in (isinstance, issubclass):
        try:
            check(User, Type[int])
        except TypeError as error:
            assert str(error) == "Subscripted generics cannot be used with class and instance checks"
        else:
            raise AssertionError("accepted parameterized typing.Type in a class check")
    print("typing.Type")
