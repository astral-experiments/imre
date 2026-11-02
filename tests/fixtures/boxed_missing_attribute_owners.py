import sys


class Journal:
    pass


def current():
    module = sys.modules[__name__]
    Journal.__name__ = 'Journáux'
    for owner, label in (
        (module, "module '" + module.__name__ + "'"),
        (Journal, "type object 'Journáux'"),
        (Journal(), "'Journáux' object"),
    ):
        try:
            getattr(owner, 'missing')
        except AttributeError as error:
            assert str(error) == label + " has no attribute 'missing'"
        else:
            raise AssertionError('lookup accepted a missing attribute')

    Journal.__name__ = '☃' * 34
    try:
        getattr(Journal, 'missing')
    except AttributeError as error:
        assert str(error) == "type object '" + '☃' * 33 + "' has no attribute 'missing'"
    else:
        raise AssertionError('lookup accepted a missing attribute')

    module.__getattr__ = lambda name: ('from hook', name)
    try:
        assert module.missing == ('from hook', 'missing')
    finally:
        del module.__getattr__
