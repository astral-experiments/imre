"""Run the complete, unchanged upstream tempfile suite with one-line result IDs."""
import sys
import unittest
from test import test_tempfile

suite = unittest.defaultTestLoader.loadTestsFromModule(test_tempfile)
result = unittest.TextTestRunner(verbosity=2, descriptions=False).run(suite)
sys.exit(0 if result.wasSuccessful() else 1)
