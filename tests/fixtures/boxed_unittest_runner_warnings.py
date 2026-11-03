import contextlib
import io
import sys
import types
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

    class Noisy(unittest.TestCase):
        def test_warning(self):
            warnings.warn("program warning", DeprecationWarning)
            self.fail("deliberate failure")

    module = types.ModuleType("warning_tests")
    module.Noisy = Noisy
    with contextlib.redirect_stderr(io.StringIO()), warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("ignore")
        program = unittest.main(module=module, argv=["runner"], exit=False, warnings="always")
        assert type(program) is unittest.TestProgram is unittest.main
        assert program.result.testsRun == 1 and len(program.result.failures) == 1
        warnings.warn("outside the run", DeprecationWarning)
        assert [str(w.message) for w in caught] == ["program warning"]

        del module.Noisy
        empty = unittest.main(module=module, argv=["runner"], exit=False)
        assert empty.result.testsRun == 0 and empty.result.wasSuccessful()
        assert empty.warnings == (None if sys.warnoptions else "default")
        assert empty.failfast is False and empty.buffer is False
