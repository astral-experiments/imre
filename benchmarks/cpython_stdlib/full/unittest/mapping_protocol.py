"""Compare formatting and failure dispatch for native string dictionaries."""
import unittest


class Problem(Exception):
    pass


class Case(unittest.TestCase):
    failureException = Problem


def compare(case: Case, left: dict[str, str], right: dict[str, str]) -> None:
    try:
        case.assertEqual(left, right, "mapping differs")
    except Problem as exc:
        print(str(exc))
    else:
        raise AssertionError("unequal dictionaries passed")


case = Case()
case.assertEqual({"b": "second", "a": "first"}, {"a": "first", "b": "second"})
# This entire entry uses only the generic method. Reachable builds must retain
# its dictionary implementation even when the caller never names that method.
empty: dict[str, str] = {}
case.assertEqual(empty, {})
compare(case, {}, {"key": "value"})
compare(case, {"different": "a"}, {"missing": "a"})
compare(case, {"z": "old", "a": "unchanged"}, {"a": "unchanged", "z": "new"})
compare(case, {"long key": "word \u2003" * 20 + "end"}, {"long key": "word \u2003" * 20 + "END"})
case.maxDiff = 0
compare(case, {"lines": "before\nafter"}, {"lines": "before\nelse"})
case.longMessage = False
compare(case, {"a": "old"}, {"a": "new"})
