import leaf_poison
import logging
import unittest
from test import support

class BufferingFormatterTest(unittest.TestCase):
    def setUp(self):
        self.records = [
            logging.makeLogRecord({'msg': 'one'}),
            logging.makeLogRecord({'msg': 'two'}),
        ]

    def test_default(self):
        f = logging.BufferingFormatter()
        self.assertEqual('', f.format([]))
        self.assertEqual('onetwo', f.format(self.records))


def setUpModule():
    unittest.enterModuleContext(support.run_with_locale('LC_ALL', ''))


if __name__ == "__main__":
    unittest.main()
