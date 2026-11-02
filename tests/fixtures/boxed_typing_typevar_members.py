from typing import TypeVar


def current():
    variable = TypeVar("T", covariant=True)
    for name in ("__covariant__", "__contravariant__", "__infer_variance__"):
        member = getattr(TypeVar, name)
        assert type(member).__name__ == "member_descriptor"
        assert member.__objclass__ is TypeVar and member.__name__ == name
        assert member.__get__(None, TypeVar) is member
        assert member.__get__(variable, TypeVar) is (name == "__covariant__")
        assert type(member).__get__(member, variable) is getattr(variable, name)
        for write in (lambda: setattr(variable, name, False),
                      lambda: delattr(variable, name)):
            try:
                write()
            except AttributeError as error:
                assert str(error) == "readonly attribute"
            else:
                raise AssertionError("variance flag was writable")

    class Other:
        flag = TypeVar.__covariant__

    assert Other.flag is TypeVar.__covariant__
    try:
        Other().flag = False
    except TypeError as error:
        print(error)
    else:
        raise AssertionError("member accepted unrelated receiver")
