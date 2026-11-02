"""Imports execute under the prompt's output capture and use its shared globals.

>>> import sys
>>> "prompt_helpers" in sys.modules
False
>>> from bad import  # doctest: +SKIP
>>> def answer():
...     from prompt_helpers import constants
...     return constants.value
>>> answer()
initializing helper
42
>>> answer()
42
>>> from decimal import Decimal
>>> str(Decimal("1.2300"))
'1.2300'
"""
import doctest

suite = doctest.DocTestSuite()
assert suite.countTestCases() == 1
for case in suite:
    case.runTest()
print("checked prompt imports")
