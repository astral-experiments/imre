import gc
import io
import os
import signal
import unittest
import weakref
from unittest import installHandler, registerResult, removeHandler, removeResult, signals
from unittest.signals import registerResult as register, removeResult as remove


def current():
    assert registerResult is signals.registerResult is register
    assert removeResult is signals.removeResult is remove
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

    assert installHandler is signals.installHandler
    assert removeHandler is signals.removeHandler
    previous = signal.getsignal(signal.SIGINT)
    registerResult(first)
    try:
        installHandler()
        os.kill(os.getpid(), signal.SIGINT)
        assert first.shouldStop
    finally:
        removeHandler()
        removeResult(first)
    assert signal.getsignal(signal.SIGINT) == previous


if __name__ == "__main__":
    current()
