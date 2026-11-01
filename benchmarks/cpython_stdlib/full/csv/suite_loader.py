from test import test_csv
import unittest


def discovery_ids(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            discovery_ids(item)
        else:
            print("IMRE_DISCOVERY_ID " + item.id())


loader = unittest.defaultTestLoader
suite = loader.loadTestsFromModule(test_csv)
if loader.errors:
    raise RuntimeError("\n".join(loader.errors))
discovery_ids(suite)
print("IMRE_DISCOVERY_COUNT", suite.countTestCases())
result = unittest.TextTestRunner(verbosity=2).run(suite)
if not result.wasSuccessful():
    raise SystemExit(1)
