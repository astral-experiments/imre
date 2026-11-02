import gc
import io
import unittest
import weakref
from unittest import registerResult, removeResult, signals


def current():
    assert registerResult is signals.registerResult
    assert removeResult is signals.removeResult
    first = unittest.TestResult()
    assert registerResult(first) is None
    assert first in signals._results
    assert removeResult(first) is True
    assert removeResult(first) is False

    seen = []
    unittest.runner.registerResult = seen.append
    try:
        observed = unittest.TextTestRunner(stream=io.StringIO()).run(unittest.TestSuite())
        assert seen == [observed]
    finally:
        unittest.runner.registerResult = registerResult

    result = unittest.TextTestRunner(stream=io.StringIO()).run(unittest.TestSuite())
    assert result in signals._results
    ref = weakref.ref(result)
    del result
    gc.collect()
    assert ref() is None
