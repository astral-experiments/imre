import _pydatetime
import asyncio
import datetime


class Flag:
    def __init__(self, allowed):
        self.allowed = allowed

    def __bool__(self):
        print("truth", self.allowed)
        return self.allowed


def message(fail):
    print("message", fail)
    if fail:
        raise ValueError("message failed")
    return ("payload", 3)


def check(flag, fail):
    assert flag, message(fail)


def exercise():
    print("stdlib", datetime.date(2024, 3, 1).isoformat(), asyncio.isfuture(None))
    print("month", _pydatetime._days_before_month(2024, 3))
    try:
        _pydatetime._days_before_month(2024, 13)
    except AssertionError as error:
        print("physical", error.args)
    check(Flag(True), True)
    for fail in False, True:
        try:
            check(Flag(False), fail)
        except (AssertionError, ValueError) as error:
            print(type(error).__name__, error.args)
