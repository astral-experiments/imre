"""Native unittest operations for Imre's fixed-layout test cases."""


class SkipTest(Exception):
    pass


class TestCase:
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
            raise AssertionError("None")
        raise AssertionError(msg)

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

    def setUp(self) -> None:
        pass

    def tearDown(self) -> None:
        pass


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
