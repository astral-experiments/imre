import sys
import warnings
from warnings import warn


def emit():
    warn("from the caller", DeprecationWarning, 2)


def current():
    assert warn is warnings.warn
    with warnings.catch_warnings(record=True) as captured:
        warnings.simplefilter("always", DeprecationWarning)
        line = sys._getframe().f_lineno + 1
        emit()
        assert len(captured) == 1
        warning = captured[0]
        assert warning.category is DeprecationWarning
        assert str(warning.message) == "from the caller"
        assert warning.filename == __file__ and warning.lineno == line

    with warnings.catch_warnings():
        warnings.simplefilter("error", UserWarning)
        try:
            warnings.warn("raised by the filter", UserWarning)
        except UserWarning as error:
            assert str(error) == "raised by the filter"
        else:
            raise AssertionError("error filter did not raise UserWarning")
