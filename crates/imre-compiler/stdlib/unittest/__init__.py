"""Native unittest operations for Imre's fixed-layout test cases."""


from types import ModuleType
from typing import ClassVar


class SkipTest(Exception):
    pass


class TestCase:
    failureException: ClassVar[type[Exception]] = AssertionError
    longMessage: bool

    def __init__(self) -> None:
        self.longMessage = True

    def countTestCases(self) -> int:
        return 1

    def _formatMessage(self, msg: str | None, standardMsg: str) -> str:
        if msg is None:
            return standardMsg
        if self.longMessage:
            return standardMsg + " : " + msg
        return msg or standardMsg

    def fail(self, msg: str | None = None) -> None:
        if msg is None:
            raise self.failureException("None")
        raise self.failureException(msg)

    def skipTest(self, reason: str) -> None:
        raise SkipTest(reason)

    def assertTrue(self, expr: bool, msg: str | None = None) -> None:
        if not expr:
            self.fail(self._formatMessage(msg, repr(expr) + " is not true"))

    def assertFalse(self, expr: bool, msg: str | None = None) -> None:
        if expr:
            self.fail(self._formatMessage(msg, repr(expr) + " is not false"))

    def assertLess(self, a: int, b: int, msg: str | None = None) -> None:
        if not a < b:
            self.fail(self._formatMessage(msg, f"{a!r} not less than {b!r}"))

    def assertLessEqual(self, a: int, b: int, msg: str | None = None) -> None:
        if not a <= b:
            self.fail(self._formatMessage(msg, f"{a!r} not less than or equal to {b!r}"))

    def assertGreater(self, a: int, b: int, msg: str | None = None) -> None:
        if not a > b:
            self.fail(self._formatMessage(msg, f"{a!r} not greater than {b!r}"))

    def assertGreaterEqual(self, a: int, b: int, msg: str | None = None) -> None:
        if not a >= b:
            self.fail(self._formatMessage(msg, f"{a!r} not greater than or equal to {b!r}"))

    def _baseAssertEqual(self, first: int, second: int, msg: str | None = None) -> None:
        if first != second:
            self.fail(self._formatMessage(msg, f"{first!r} != {second!r}"))

    def assertEqual(self, first: int, second: int, msg: str | None = None) -> None:
        self._baseAssertEqual(first, second, msg)

    def assertNotEqual(self, first: int, second: int, msg: str | None = None) -> None:
        if first == second:
            self.fail(self._formatMessage(msg, f"{first!r} == {second!r}"))

    def assertHasAttr(self, obj: ModuleType, name: str, msg: str | None = None) -> None:
        if not hasattr(obj, name):
            self.fail(self._formatMessage(msg, "module " + repr(obj.__name__) + " has no attribute " + repr(name)))

    def assertIn(self, member: object, container: dict[int, str], msg: str | None = None) -> None:
        if member not in container:
            self.fail(self._formatMessage(msg, repr(member) + " not found in " + repr(container)))

    def setUp(self) -> None:
        pass

    def tearDown(self) -> None:
        pass


def _checked_expected(kinds: list[int], error: str | None) -> list[int]:
    if error is not None:
        raise TypeError(error)
    return kinds


class _AssertRaisesContext:
    test_case: TestCase
    msg: str | None
    _expected: list[int]
    _label: str
    _caught: Exception | None

    def __init__(self, case: TestCase, kinds: list[int], label: str, msg: str | None, error: str | None) -> None:
        self.test_case = case
        self._expected = _checked_expected(kinds, error)
        self._label = label
        self.msg = msg
        self._caught = None

    def _missing(self) -> None:
        raise self.test_case.failureException(self.test_case._formatMessage(self.msg, self._label + " not raised"))

    @property
    def exception(self) -> Exception:
        caught = self._caught
        if caught is None:
            raise AttributeError("'_AssertRaisesContext' object has no attribute 'exception'")
        return caught


class TestResult:
    testsRun: int
    shouldStop: bool
    failfast: bool
    skipped: list[tuple[TestCase, str]]
    unexpectedSuccesses: list[TestCase]

    def __init__(self) -> None:
        self.testsRun = 0
        self.shouldStop = False
        self.failfast = False
        self.skipped = []
        self.unexpectedSuccesses = []

    def startTestRun(self) -> None:
        pass

    def stopTestRun(self) -> None:
        pass

    def startTest(self, test: TestCase) -> None:
        self.testsRun += 1

    def stopTest(self, test: TestCase) -> None:
        pass

    def addSuccess(self, test: TestCase) -> None:
        pass

    def addSkip(self, test: TestCase, reason: str) -> None:
        self.skipped.append((test, reason))

    def addUnexpectedSuccess(self, test: TestCase) -> None:
        if self.failfast:
            self.stop()
        self.unexpectedSuccesses.append(test)

    def stop(self) -> None:
        self.shouldStop = True
