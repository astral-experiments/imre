"""Run the intact datetime registrations with the frozen regrtest resources."""
import datetime_poison
from test import support
import unittest

support.use_resources = []

if __name__ == "__main__":
    unittest.main(
        module="test.test_datetime",
        testRunner=unittest.TextTestRunner(verbosity=2, descriptions=False),
    )
