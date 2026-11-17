from test import support
import sys
import unittest


def main():
    support.use_resources = {}
    from test import test_weakref, test_weakset
    loader = unittest.defaultTestLoader
    suite = unittest.TestSuite()
    with open(sys.argv[1]) as selectors:
        for name in selectors:
            suite.addTests(loader.loadTestsFromName(name.strip()))
    if loader.errors:
        raise RuntimeError("\n".join(loader.errors))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful():
        raise SystemExit(1)


main()
