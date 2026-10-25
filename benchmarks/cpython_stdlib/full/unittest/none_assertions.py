from unittest import TestCase


class Marker(ValueError):
    pass


class CustomCase(TestCase):
    failureException = Marker


class Trace:
    events: str

    def __init__(self) -> None:
        self.events = ""

    def value(self) -> bool:
        self.events = self.events + "value "
        return False

    def message(self) -> str:
        self.events = self.events + "message"
        return "detail"


def optional(present: bool) -> str | None:
    if present:
        return "None"
    return None


def try_none(case: TestCase, value: object) -> None:
    try:
        case.assertIsNone(value)
    except AssertionError as exc:
        print(str(exc))
    else:
        print("is None")


case = TestCase()
try_none(case, optional(False))
try_none(case, optional(True))
try_none(case, 0)
try_none(case, b"a")
try_none(case, (1, "two"))
case.assertIsNotNone(False)
case.assertIsNotNone("")

subclass = CustomCase()
trace = Trace()
try:
    subclass.assertIsNone(obj=trace.value(), msg=trace.message())
except Marker as exc:
    print(str(exc))
print(trace.events)
subclass.longMessage = False
try:
    subclass.assertIsNotNone(obj=None, msg="replacement")
except Marker as exc:
    print(str(exc))
