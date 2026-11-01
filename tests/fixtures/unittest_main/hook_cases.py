import unittest
from lifecycle import Outcomes
from lifecycle import Outcomes as Repeated


def select(loader, standard_tests, pattern):
    print(pattern)
    print(standard_tests.countTestCases())
    selected = unittest.TestSuite()
    selected.addTests(standard_tests)
    return selected


def load_tests(*args):
    return select(*args)
