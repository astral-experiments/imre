import sys
import unittest

from annotationlib import Format, ForwardRef
from test import test_annotationlib as originals


key = "test.test_annotationlib.TestForwardRefClass.test_evaluate_forwardref_format"
evaluate = ForwardRef.evaluate


def wrong(self, **kwargs):
    value = evaluate(self, **kwargs)
    if self.__forward_arg__ == "a + b" and "locals" in kwargs:
        assert kwargs["format"] == Format.FORWARDREF
        assert kwargs["locals"] == {"a": 1, "b": 2}
        assert type(value) is int and value == 3
        print("OBSERVED", self.__forward_arg__, kwargs["locals"], value, flush=True)
        print("WRONG_ACTUAL", 4, flush=True)
        return 4
    return value


if __name__ == "__main__":
    mode = sys.argv[1]
    assert mode in ("pass", "wrong")
    print("BACKEND", sys.implementation.name, flush=True)
    owner = originals.TestForwardRefClass
    cases = [t for t in unittest.defaultTestLoader.loadTestsFromTestCase(owner) if t.id() == key]
    assert len(cases) == 1
    print("REGISTERED_ID", cases[0].id(), flush=True)
    print("REGISTERED_COUNT", len(cases), flush=True)
    try:
        if mode == "wrong":
            ForwardRef.evaluate = wrong
        result = unittest.TextTestRunner(verbosity=2, descriptions=False).run(unittest.TestSuite(cases))
    finally:
        if mode == "wrong":
            ForwardRef.evaluate = evaluate
    print("RAN", result.testsRun, flush=True)
    sys.exit(0 if result.wasSuccessful() else 1)
