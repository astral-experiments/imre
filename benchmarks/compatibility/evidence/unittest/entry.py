from test import test_unittest
import sys
import unittest

loader = unittest.defaultTestLoader
if len(sys.argv) > 1:
    suite = loader.loadTestsFromName(sys.argv[1])
else:
    suite = loader.loadTestsFromModule(test_unittest)
if loader.errors:
    raise RuntimeError("\n".join(loader.errors))
result = unittest.TextTestRunner(verbosity=2).run(suite)
if not result.wasSuccessful():
    raise SystemExit(1)
