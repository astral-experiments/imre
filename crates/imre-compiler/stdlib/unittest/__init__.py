"""Native unittest operations for Imre's fixed-layout test cases."""


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


from typing import Callable
import sys
import time


def main() -> None:
    # A direct call discovers TestCase classes from the compiled entry module.
    raise RuntimeError("native unittest.main requires a direct call")


class _MainRunner:
    total: int
    failures: list[str]
    errors: list[str]
    skips: int
    failfast: bool
    verbose: int
    started: float
    current_name: str
    name_pending: bool

    def __init__(self) -> None:
        self.total = 0
        self.failures = []
        self.errors = []
        self.skips = 0
        self.failfast = False
        self.verbose = 1
        self.current_name = ""
        self.name_pending = False
        self.started = time.time()

    def _start(self) -> None:
        for flag in sys.argv[1:]:
            if flag == "-v" or flag == "--verbose":
                self.verbose = 2
            elif flag == "-q" or flag == "--quiet":
                self.verbose = 0
            elif flag == "-f" or flag == "--failfast":
                self.failfast = True
            else:
                print("unsupported unittest argument: " + flag, file=sys.stderr)
                sys.exit(2)

    def _mark(self, short: str, long: str) -> None:
        if self.verbose == 1:
            sys.stderr.write(short)
        elif self.verbose == 2:
            if not self.name_pending:
                sys.stderr.write(self.current_name + " ... ")
            print(long, file=sys.stderr)
            self.name_pending = False

    def _phase(self, action: Callable[[], None], name: str) -> bool:
        try:
            action()
        except SkipTest as exc:
            self.skips += 1
            self._mark("s", "skipped " + repr(str(exc)))
            return False
        except AssertionError as exc:
            self.failures.append("FAIL: " + name + "\nAssertionError: " + str(exc))
            self._mark("F", "FAIL")
            return False
        except SystemExit as exc:
            self.errors.append("ERROR: " + name + "\nSystemExit: " + str(exc))
            self._mark("E", "ERROR")
            return False
        except Exception as exc:
            self.errors.append("ERROR: " + name + "\n" + str(exc))
            self._mark("E", "ERROR")
            return False
        return True

    def _run(self, name: str, setup: Callable[[], None],
             body: Callable[[], None], teardown: Callable[[], None]) -> None:
        if self.failfast and (self.errors or self.failures):
            return
        self.total += 1
        if self.verbose == 2:
            self.current_name = name
            self.name_pending = True
            sys.stderr.write(name + " ... ")
        if self._phase(setup, name):
            passed = self._phase(body, name)
            cleaned = self._phase(teardown, name)
            if passed and cleaned:
                self._mark(".", "ok")

    def _finish(self) -> None:
        if self.verbose:
            print("", file=sys.stderr)
        for detail in self.errors + self.failures:
            print("======================================================================", file=sys.stderr)
            print(detail, file=sys.stderr)
        elapsed = time.time() - self.started
        plural = "" if self.total == 1 else "s"
        print("----------------------------------------------------------------------", file=sys.stderr)
        print(f"Ran {self.total} test{plural} in {elapsed:.3f}s\n", file=sys.stderr)
        parts: list[str] = []
        if self.failures:
            parts.append("failures=" + str(len(self.failures)))
        if self.errors:
            parts.append("errors=" + str(len(self.errors)))
        if self.skips:
            parts.append("skipped=" + str(self.skips))
        message = "FAILED" if self.failures or self.errors else "OK"
        if parts:
            message += " (" + ", ".join(parts) + ")"
        if not self.total:
            print("NO TESTS RAN", file=sys.stderr)
            sys.exit(5)
        print(message, file=sys.stderr)
        if self.failures or self.errors:
            sys.exit(1)
        sys.exit(0)
