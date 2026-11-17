"""Run an intact unittest selector with compact, exact-ID result descriptions."""
import unittest

class ReceiptResult(unittest.TextTestResult):
    def getDescription(self, test):
        test_id = test.id()
        return f"{test_id.rsplit('.', 1)[-1]} ({test_id})"

    def printErrors(self):
        if self.dots or self.showAll:
            self.stream.writeln()
        for label, entries in (("ERROR", self.errors), ("FAIL", self.failures)):
            for test, detail in entries:
                self.stream.writeln(f"{label}: {test.id()}")
                lines = detail.rstrip().splitlines()
                for line in lines[-3:]:
                    self.stream.writeln(line[:500])

if __name__ == '__main__':
    unittest.main(module=None, testRunner=unittest.TextTestRunner(verbosity=2, resultclass=ReceiptResult))
