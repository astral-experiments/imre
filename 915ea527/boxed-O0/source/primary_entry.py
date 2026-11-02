import sys
import unittest
from test import test_io

if __name__ == "__main__":
    wanted = (
        "test.test_io.CBufferedRWPairTest.test_constructor_max_buffer_size_removal",
        "test.test_io.CBufferedRWPairTest.test_constructor_with_not_readable",
        "test.test_io.CBufferedRWPairTest.test_constructor_with_not_writeable",
        "test.test_io.CBufferedRandomTest.test_args_error",
        "test.test_io.CBufferedRandomTest.test_truncate",
    )
    suite = unittest.defaultTestLoader.loadTestsFromModule(test_io)
    print("registered", suite.countTestCases(), flush=True)

    def walk_loaded(suite, mode, selected):
        for case in suite:
            if isinstance(case, unittest.TestSuite):
                walk_loaded(case, mode, selected)
            else:
                ident = case.id()
                if mode == "list":
                    print(ident)
                elif ident in wanted:
                    selected.append(case)

    mode = sys.argv[1]
    if mode == "list":
        walk_loaded(suite, mode, [])
    else:
        if mode == "selected":
            selected = []
            walk_loaded(suite, mode, selected)
            print("selected", len(selected), flush=True)
            if len(selected) != len(wanted):
                raise AssertionError("original registered case missing")
            suite = unittest.TestSuite(selected)
        elif mode != "whole":
            raise ValueError(mode)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        print("completed", result.testsRun, len(result.failures), len(result.errors), len(result.skipped), flush=True)
        if not result.wasSuccessful():
            raise SystemExit(1)
