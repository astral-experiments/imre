"""Check TestCase equality across bool and int, including the exact failure text."""

import unittest


class SpecificFailure(AssertionError):
    pass


class SpecificCase(unittest.TestCase):
    failureException = SpecificFailure


case = SpecificCase()
case.assertEqual(True, 1)
case.assertEqual(0, False)
case.assertEqual(False, False)
case.assertEqual(second=True, first=1)
try:
    case.assertEqual(True, False)
except SpecificFailure as exc:
    print(str(exc))
try:
    case.assertEqual(True, 2, "bound")
except SpecificFailure as exc:
    print(str(exc))
try:
    case.assertEqual(-1, False, "checked")
except SpecificFailure as exc:
    print(str(exc))
try:
    case.assertEqual(False, 10 ** 102)
except SpecificFailure as exc:
    print(str(exc))
case.longMessage = False
try:
    case.assertEqual(False, True, msg="short")
except SpecificFailure as exc:
    print(str(exc))
print("completed")
