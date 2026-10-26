"""Check TestCase truth assertions, the original failure value and call order."""
from unittest import TestCase


class Failure(Exception):
    pass


class Custom(TestCase):
    failureException = Failure

    def fail(self, msg: str | None = None) -> None:
        raise RuntimeError("truth assertions do not call fail")


def check(case: TestCase, value: object) -> None:
    for expected in (False, True):
        try:
            if expected:
                case.assertTrue(value)
            else:
                case.assertFalse(value)
            print("pass " + str(expected))
        except AssertionError as failed:
            print(str(failed))


def maybe(value: bool | None) -> None:
    case = TestCase()
    case.assertFalse(value)


events = [0]


def operand() -> int:
    print("operand " + str(events[0]))
    return events[0]


def message() -> str:
    print("message")
    events[0] = 1
    return "receipt"


case = TestCase()
check(case, None)
check(case, False)
check(case, 0)
check(case, -7)
check(case, "")
check(case, "ß")
check(case, b"")
check(case, b"\xff")
check(case, ())
check(case, (False, ("x", 2)))
maybe(None)
maybe(False)
try:
    maybe(True)
except AssertionError as failed:
    print(str(failed))

try:
    case.assertTrue(expr=operand(), msg=message())
except AssertionError as failed:
    print(str(failed))
events[0] = 0
case.assertTrue(msg=message(), expr=operand())

custom = Custom()
false = custom.assertFalse
try:
    false("value", "context")
except Failure as failed:
    print(str(failed))
custom.longMessage = False
try:
    custom.assertTrue((), msg="only the custom message")
except Failure as failed:
    print(str(failed))
