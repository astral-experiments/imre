import sys
import warnings
from warnings import warn


class Notice(UserWarning):
    pass


def caller(message):
    warn(message, stacklevel=2)


def current():
    original_filters = warnings.filters
    original_show = warnings.showwarning
    message = Notice("source")
    with warnings.catch_warnings(record=True) as outer:
        warnings.simplefilter("always", (ResourceWarning, (UserWarning,)))
        expected_line = sys._getframe().f_lineno + 1
        caller(message)
        event, = outer
        assert event.category is Notice and event.message is message
        assert event.filename == __file__ and event.lineno == expected_line
        try:
            with warnings.catch_warnings(record=True) as inner:
                warnings.simplefilter("ignore", ResourceWarning)
                warn("silent", ResourceWarning)
                warn("inner")
                assert len(inner) == 1 and type(inner[0].message) is UserWarning
                warnings.simplefilter("error", UserWarning)
                warn(message)
        except Notice as caught:
            assert caught is message
        else:
            raise AssertionError("the error filter did not raise the warning")
        warn("outside nested context")
        assert len(outer) == 2 and str(outer[-1].message) == "outside nested context"
    assert warnings.filters is original_filters and warnings.showwarning is original_show


if __name__ == "__main__":
    current()
