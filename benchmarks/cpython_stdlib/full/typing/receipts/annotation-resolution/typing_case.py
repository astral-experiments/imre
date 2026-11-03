import sys
import typing
import unittest
from test import test_typing as originals


KEY = "test.test_typing.NoTypeCheckTests.test_no_type_check_forward_ref_as_string"
resolve = originals.get_type_hints


def wrong(cls):
    value = resolve(cls)
    if cls.__name__ == "F":
        assert value == {"foo": typing.ClassVar[int]}
        print("OBSERVED F ClassVar[int]", flush=True)
        print("WRONG_ACTUAL F ClassVar[str]", flush=True)
        return {"foo": typing.ClassVar[str]}
    return value


if __name__ == "__main__":
    mode = sys.argv[1]
    assert mode in ("pass", "wrong")
    print("BACKEND", sys.implementation.name, flush=True)
    owner = originals.NoTypeCheckTests
    cases = [case for case in unittest.defaultTestLoader.loadTestsFromTestCase(owner) if case.id() == KEY]
    assert len(cases) == 1
    print("REGISTERED_ID", cases[0].id(), flush=True)
    print("REGISTERED_COUNT", len(cases), flush=True)
    try:
        if mode == "wrong":
            originals.get_type_hints = wrong
        result = unittest.TextTestRunner(verbosity=2, descriptions=False).run(unittest.TestSuite(cases))
    finally:
        if mode == "wrong":
            originals.get_type_hints = resolve
    print("RAN", result.testsRun, flush=True)
    sys.exit(0 if result.wasSuccessful() else 1)
