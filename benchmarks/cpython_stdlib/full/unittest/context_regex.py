"""Compare real exception-context outcomes and original pattern spellings."""
import re
import unittest


class OriginalFailure(Exception):
    pass


class Custom(unittest.TestCase):
    failureException = OriginalFailure


def supplied() -> re.Pattern[str]:
    return re.compile(r" ^ \s+ [0-9]+ $ ", re.VERBOSE)


def captured(case: unittest.TestCase, pattern: re.Pattern[str]) -> None:
    original = ValueError("not a number")
    context = case.assertRaisesRegex(ValueError, pattern, msg="custom")
    try:
        with context:
            raise original
    except AssertionError as failure:
        print(str(failure))
        print(context.exception is original)


def computed(case: unittest.TestCase, raw: str, message: str) -> None:
    with case.assertRaisesRegex(ValueError, raw):
        raise ValueError(message)


def mark(label: str, value: str) -> str:
    print(label)
    return value


case = unittest.TestCase()
actual = ValueError("prefix second\x1c42 suffix")
with case.assertRaisesRegex((ValueError, LookupError), r"(first|second)\s+[0-9]+") as caught:
    raise actual
print(caught.exception is actual)

with case.assertRaisesRegex(ValueError, supplied()):
    raise ValueError(" 42\n")
captured(case, supplied())
captured(case, re.compile("x"))
captured(case, re.compile(" x", re.VERBOSE))
with case.assertRaisesRegex(ValueError, mark("pattern", "n[0-9]"), msg=mark("message", "check")):
    raise ValueError("n5")
computed(case, "z[0-9]+", "before z37 after")
try:
    computed(case, "^z[0-9]+$", "before z37 after")
except AssertionError as failure:
    print(str(failure))

wrong = TypeError("the text matches")
try:
    with case.assertRaisesRegex(ValueError, "matches"):
        raise wrong
except TypeError as failure:
    print(failure is wrong)

try:
    with case.assertRaisesRegex(ValueError, "unused", msg="missing"):
        pass
except AssertionError as failure:
    print(str(failure))

custom = Custom()
custom.longMessage = False
try:
    with custom.assertRaisesRegex(TypeError, "^wanted$", msg="custom only"):
        raise TypeError("unwanted")
except OriginalFailure as failure:
    print(str(failure))

with case.assertRaisesRegex(ValueError, None):
    raise ValueError("no pattern requested")
try:
    case.assertRaisesRegex(1, "x")
except TypeError as failure:
    print(str(failure))

try:
    with case.assertRaisesRegex(ValueError, "needle", invalid_keyword=3):
        print("invalid context entered")
except TypeError as failure:
    print(str(failure))

# Argument expressions run before binding; the first duplicate names the error.
for regex_first in (False, True):
    try:
        if regex_first:
            case.assertRaisesRegex(ValueError, "x", expected_regex=mark("keyword", "y"), expected_exception=TypeError)
        else:
            case.assertRaisesRegex(ValueError, "x", expected_exception=TypeError, expected_regex=mark("keyword", "y"))
    except TypeError as failure:
        print(str(failure))
