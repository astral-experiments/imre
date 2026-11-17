import sys
import unittest
from test import support

# Match regrtest's default resource policy while keeping unittest's raw IDs on one line.
support.use_resources = []
runner = unittest.TextTestRunner(verbosity=2, descriptions=False)
unittest.main(module=None, argv=['unittest', '-v', sys.argv[1]], testRunner=runner)
