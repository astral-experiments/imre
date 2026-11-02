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
        for base in (type(form), type(alias)):
            try:
                class Child(base):
                    pass
            except TypeError as exc:
                assert str(exc) == "Cannot subclass special typing classes"
            else:
                raise AssertionError("accepted a subclass of a special typing class")

            assert base.__init_subclass__(_root=True) is None
            class Root(base, _root=True):
                pass
            assert Root.__bases__ == (base,)
        try:
            form[int, str]
        except TypeError:
            pass
        else:
            raise AssertionError("accepted two type arguments")
    assert TypeGuard[int] != TypeIs[int]
    print("type predicates")
