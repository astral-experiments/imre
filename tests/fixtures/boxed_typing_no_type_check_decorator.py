import sys
import warnings
from typing import no_type_check_decorator


def current():
    calls = []
    def factory(value, *, target):
        calls.append(value)
        return target

    def result(): pass

    with warnings.catch_warnings(record=True) as records:
        warnings.simplefilter("always")
        where = sys._getframe().f_lineno + 1
        decorated = no_type_check_decorator(factory)
    warning, = records
    assert warning.category is DeprecationWarning and warning.filename == __file__
    assert warning.lineno == where
    assert str(warning.message) == "'typing.no_type_check_decorator' is deprecated and slated for removal in Python 3.15"
    assert decorated.__wrapped__ is factory and decorated.__name__ == factory.__name__
    assert calls == [] and decorated(42, target=result) is result
    assert calls == [42] and result.__no_type_check__ is True
    print("deprecated decorator")
