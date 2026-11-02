"""Weak result registration from CPython 3.14.5 unittest.signals."""

import weakref

__unittest = True

_results = weakref.WeakKeyDictionary()
def registerResult(result):
    _results[result] = 1

def removeResult(result):
    return bool(_results.pop(result, None))
