from datetime import date, datetime, timedelta
from unittest import TestCase


class CalendarCase(TestCase):
    pass


class Validation(TestCase):
    failureException = ValueError


class Override(TestCase):
    def assertEqual(self, first: date, second: date, msg: str | None = None) -> None:
        print("override")


def value(label: str, day: int) -> date:
    print(label)
    return date(2000, 2, day)


case = CalendarCase()
case.assertEqual(date(2000, 2, 29), date(2000, 2, 29))
case.assertEqual(timedelta(days=1), timedelta(hours=24))
try:
    case.assertEqual(second=value("right", 28), first=value("left", 29), msg="leap day")
except AssertionError as exc:
    print(str(exc))
try:
    case.assertEqual(date(2000, 2, 29), datetime(2000, 2, 29))
except AssertionError as exc:
    print(str(exc))
case.longMessage = False
try:
    case.assertEqual(timedelta(days=-1), timedelta(seconds=1), "duration")
except AssertionError as exc:
    print(str(exc))
try:
    Validation().assertEqual(timedelta(microseconds=-1), timedelta(days=-1))
except ValueError as exc:
    print(str(exc))
Override().assertEqual(date(2000, 2, 29), date(2000, 2, 28))
