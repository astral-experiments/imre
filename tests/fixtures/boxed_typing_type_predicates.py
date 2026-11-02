import typing
from typing import TypeGuard, TypeIs


def current():
    for form in (TypeGuard, TypeIs):
        assert getattr(typing, form.__name__) is form
        alias = form[int]
        assert typing.get_origin(alias) is form
        assert typing.get_args(alias) == (int,)
        assert alias is form[int]
        assert form[int] != form[str]
        try:
            form[int, str]
        except TypeError:
            pass
        else:
            raise AssertionError("accepted two type arguments")
    assert TypeGuard[int] != TypeIs[int]
    print("type predicates")
