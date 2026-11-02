import io
import unittest
import warnings


def current():
    def noisy(result):
        warnings.warn("inside", UserWarning)

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        for action in ("ignore", "always"):
            unittest.TextTestRunner(stream=io.StringIO(), warnings=action).run(noisy)
        assert [str(w.message) for w in caught] == ["inside"]
        try:
            unittest.TextTestRunner(stream=io.StringIO(), warnings="error").run(noisy)
        except UserWarning as error:
            assert str(error) == "inside"
        else:
            raise AssertionError("runner ignored the error filter")
        warnings.warn("restored", UserWarning)
        assert [str(w.message) for w in caught] == ["inside", "restored"]
