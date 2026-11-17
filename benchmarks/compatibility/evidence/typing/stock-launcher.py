import sys
import unittest
from test import support

support.use_resources = []
class ExactIDResult(unittest.TextTestResult):
    def getDescription(self, test):
        ident = test.id()
        return ident.rsplit('.', 1)[-1] + ' (' + ident + ')'
runner = unittest.TextTestRunner(verbosity=2, descriptions=False, resultclass=ExactIDResult)
unittest.main(module=None, argv=['unittest', '-v', sys.argv[1]], testRunner=runner)
