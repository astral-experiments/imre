"""Native unittest operations for Imre's fixed-layout test cases."""


import difflib
import sys
import time
from typing import Callable, ClassVar


class SkipTest(Exception):
    pass


def _shorten(text: str, begin: int, end: int) -> str:
    skipped = len(text) - begin - end
    if skipped <= 12:
        return text
    return text[:begin] + "[" + str(skipped) + " chars]" + text[len(text) - end:]


def _short_reprs(left: str, right: str) -> tuple[str, str]:
    longest = max(len(left), len(right))
    if longest <= 80:
        return (left, right)
    prefix = 0
    while prefix < len(left) and prefix < len(right) and left[prefix] == right[prefix]:
        prefix += 1
    common = 80 - (longest - prefix + 17)
    if common > 5:
        shared = _shorten(left[:prefix], 5, common)
        return (shared + left[prefix:], shared + right[prefix:])
    shared = _shorten(left[:prefix], 5, 5)
    return (shared + _shorten(left[prefix:], 41, 5), shared + _shorten(right[prefix:], 41, 5))


def _kept_lines(text: str) -> list[str]:
    lines: list[str] = []
    start = 0
    index = 0
    while index < len(text):
        char = text[index]
        index += 1
        if char in "\r\n\v\f\x1c\x1d\x1e\x85\u2028\u2029":
            if char == "\r" and index < len(text) and text[index] == "\n":
                index += 1
            lines.append(text[start:index])
            start = index
    if start < len(text):
        lines.append(text[start:])
    return lines


def _pretty_text_item(value: str) -> str:
    # pprint.pformat(list) indents each element by one column. A long
    # string keeps each whitespace run attached to its preceding word.
    if len(repr(value)) <= 78:
        return repr(value)
    chunks: list[str] = []
    lines = _kept_lines(value)
    spaces = "\t\n\v\f\r\x1c\x1d\x1e\x1f \x85\xa0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000"
    for index in range(len(lines)):
        line = lines[index]
        final = index == len(lines) - 1
        if len(repr(line)) <= (78 if final else 79):
            chunks.append(repr(line))
        else:
            pos = 0
            current = ""
            while pos < len(line):
                start = pos
                while pos < len(line) and line[pos] not in spaces:
                    pos += 1
                while pos < len(line) and line[pos] in spaces:
                    pos += 1
                part = line[start:pos]
                candidate = current + part
                if len(repr(candidate)) > (78 if final and pos == len(line) else 79):
                    if current:
                        chunks.append(repr(current))
                    current = part
                else:
                    current = candidate
            if current:
                chunks.append(repr(current))
    return "\n ".join(chunks)


def _pretty_bytes_item(value: bytes) -> str:
    if len(repr(value)) <= 78:
        return repr(value)
    chunks: list[str] = []
    current = b""
    for i in range(0, len(value), 4):
        part = value[i:i + 4]
        candidate = current + part
        if len(repr(candidate)) > (78 if i == len(value) // 4 * 4 else 79):
            if current:
                chunks.append(repr(current))
            current = part
        else:
            current = candidate
    if current:
        chunks.append(repr(current))
    return "\n ".join(chunks)


def _list_repr(items: list[str]) -> str:
    return "[" + ", ".join(items) + "]"


def _pretty_list(flat: str, items: list[str]) -> list[str]:
    if len(flat) <= 80:
        return [flat]
    return ("[" + ",\n ".join(items) + "]").splitlines()


class _SuiteItem:
    def _as_suite(self) -> TestSuite | None:
        return None

    def countTestCases(self) -> int:
        return 1

    def _run_item(self, result: TestResult) -> None:
        raise TypeError("test item has no run method")

    def _run_leaf(self, result: TestResult) -> None:
        raise TypeError("expected a test case")

    def _leaf_count(self) -> int:
        return 1


class TestCase(_SuiteItem):
    failureException: ClassVar[type[Exception]] = AssertionError
    longMessage: bool
    _testMethodName: str
    _testClassName: str
    maxDiff: int | None
    _diffThreshold: int

    def __init__(self, methodName: str = "runTest") -> None:
        self.longMessage = True
        self.maxDiff = 640
        self._diffThreshold = 65536
        self._testMethodName = methodName
        self._testClassName = "unittest.case.TestCase"

    def id(self) -> str:
        return self._testClassName + "." + self._testMethodName

    def _description(self) -> str:
        return self._testMethodName + " (" + self.id() + ")"

    def _missing_method(self) -> None:
        raise AttributeError("no such test method: " + self._testMethodName)

    def _invoke_test(self) -> None:
        self._missing_method()

    def _run_item(self, result: TestResult) -> None:
        self.run(result)

    def _run_leaf(self, result: TestResult) -> None:
        self.run(result)

    def _leaf_count(self) -> int:
        return self.countTestCases()

    def _record_exception(self, result: TestResult, exc: Exception) -> None:
        if isinstance(exc, self.failureException):
            result._failure(self, str(exc))
        else:
            result._error(self, str(exc))

    def run(self, result: TestResult | None = None) -> TestResult:
        own = result is None
        if result is None:
            result = TestResult()
            result.startTestRun()
        result.startTest(self)
        passed = True
        try:
            try:
                self.setUp()
            except SkipTest as exc:
                result.addSkip(self, str(exc))
                passed = False
            except Exception as exc:
                self._record_exception(result, exc)
                passed = False
            if passed:
                try:
                    self._invoke_test()
                except SkipTest as exc:
                    result.addSkip(self, str(exc))
                    passed = False
                except Exception as exc:
                    self._record_exception(result, exc)
                    passed = False
                try:
                    self.tearDown()
                except SkipTest as exc:
                    result.addSkip(self, str(exc))
                    passed = False
                except Exception as exc:
                    self._record_exception(result, exc)
                    passed = False
            if passed:
                result.addSuccess(self)
        finally:
            result.stopTest(self)
            if own:
                result.stopTestRun()
        return result

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

    def _raise_missing_callable(self, kind: str, name: str) -> None:
        raise self.failureException(self._formatMessage(None, kind + " not raised by " + name))

    def _bound_assertEqual(self) -> _BoundEqual:
        return _BoundEqual(self)

    def _bound_assertListEqual(self) -> _BoundListEqual:
        return _BoundListEqual(self)

    def _bound_assertRaises(self) -> _BoundRaises:
        return _BoundRaises(self)

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

    def _assertBoolEqual(self, first: bool, second: bool, msg: str | None = None) -> None:
        if first != second:
            self.fail(self._formatMessage(msg, f"{first!r} != {second!r}"))

    def _assertBoolIntEqual(self, first: bool, second: int, msg: str | None = None) -> None:
        if int(first) != second:
            self.fail(self._formatMessage(msg, f"{first!r} != {second!r}"))

    def _assertIntBoolEqual(self, first: int, second: bool, msg: str | None = None) -> None:
        if first != int(second):
            self.fail(self._formatMessage(msg, f"{first!r} != {second!r}"))

    def _failMembership(self, member: str, container: str, msg: str | None, expected: bool) -> None:
        detail = " not found in " if expected else " unexpectedly found in "
        self.fail(self._formatMessage(msg, member + detail + container))

    def _assertTextMembership(self, member: str, container: str, msg: str | None = None, *, expected: bool) -> None:
        if (member in container) != expected:
            self._failMembership(repr(member), repr(container), msg, expected)

    def _assertTextListMembership(self, member: str, container: list[str], msg: str | None = None, *, expected: bool) -> None:
        if (member in container) != expected:
            self._failMembership(repr(member), repr(container), msg, expected)

    def _assertIntListMembership(self, member: int, container: list[int], msg: str | None = None, *, expected: bool) -> None:
        found = False
        for item in container:
            if member == item:
                found = True
                break
        if found != expected:
            self._failMembership(repr(member), repr(container), msg, expected)

    def _assertTextDictMembership(self, member: str, container: dict[str, str], msg: str | None = None, *, expected: bool) -> None:
        if (member in container) != expected:
            self._failMembership(repr(member), repr(container), msg, expected)

    def _truncateMessage(self, message: str, diff: str) -> str:
        limit = self.maxDiff
        if limit is None or len(diff) <= limit:
            return message + diff
        return message + "\nDiff is " + str(len(diff)) + " characters long. Set self.maxDiff to None to see it."

    def assertMultiLineEqual(self, first: str, second: str, msg: str | None = None) -> None:
        if first == second:
            return
        sides = _short_reprs(repr(first), repr(second))
        standard = sides[0] + " != " + sides[1]
        if len(first) <= self._diffThreshold and len(second) <= self._diffThreshold:
            # Append to both when either lacks a newline; the diff must show
            # an extra newline when only one original operand ends in one.
            if first and second:
                if first[-1] != "\n" or second[-1] != "\n":
                    first += "\n"
                    second += "\n"
            elif first and first[-1] != "\n":
                first += "\n"
            elif second and second[-1] != "\n":
                second += "\n"
            changes = list(difflib.ndiff(_kept_lines(first), _kept_lines(second)))
            standard = self._truncateMessage(standard, "\n" + "".join(changes))
        self.fail(self._formatMessage(msg, standard))

    def _list_difference(self, left: list[str], right: list[str], pretty1: list[str],
                         pretty2: list[str], msg: str | None) -> None:
        flat1 = _list_repr(left)
        flat2 = _list_repr(right)
        sides = _short_reprs(flat1, flat2)
        message = "Lists differ: " + sides[0] + " != " + sides[1] + "\n"
        for i in range(min(len(left), len(right))):
            if left[i] != right[i]:
                difference = _short_reprs(left[i], right[i])
                message += "\nFirst differing element " + str(i) + ":\n" + difference[0] + "\n" + difference[1] + "\n"
                break
        if len(left) > len(right):
            message += "\nFirst list contains " + str(len(left) - len(right)) + " additional elements.\n"
            message += "First extra element " + str(len(right)) + ":\n" + left[len(right)] + "\n"
        elif len(right) > len(left):
            message += "\nSecond list contains " + str(len(right) - len(left)) + " additional elements.\n"
            message += "First extra element " + str(len(left)) + ":\n" + right[len(left)] + "\n"
        changes = list(difflib.ndiff(_pretty_list(flat1, pretty1), _pretty_list(flat2, pretty2)))
        self.fail(self._formatMessage(msg, self._truncateMessage(message, "\n" + "\n".join(changes))))

    def _assertTextListEqual(self, list1: list[str], list2: list[str], msg: str | None = None) -> None:
        same = len(list1) == len(list2)
        if same:
            for index in range(len(list1)):
                if list1[index] != list2[index]:
                    same = False
                    break
        if not same:
            self._list_difference([repr(x) for x in list1], [repr(x) for x in list2],
                                  [_pretty_text_item(x) for x in list1], [_pretty_text_item(x) for x in list2], msg)

    def _assertBytesListEqual(self, list1: list[bytes], list2: list[bytes], msg: str | None = None) -> None:
        same = len(list1) == len(list2)
        if same:
            for index in range(len(list1)):
                if list1[index] != list2[index]:
                    same = False
                    break
        if not same:
            self._list_difference([repr(x) for x in list1], [repr(x) for x in list2],
                                  [_pretty_bytes_item(x) for x in list1], [_pretty_bytes_item(x) for x in list2], msg)

    def _assertIntListEqual(self, list1: list[int], list2: list[int], msg: str | None = None) -> None:
        same = len(list1) == len(list2)
        if same:
            for index in range(len(list1)):
                if list1[index] != list2[index]:
                    same = False
                    break
        if not same:
            left = [repr(x) for x in list1]
            right = [repr(x) for x in list2]
            self._list_difference(left, right, left, right, msg)

    def assertNotEqual(self, first: int, second: int, msg: str | None = None) -> None:
        if first == second:
            self.fail(self._formatMessage(msg, f"{first!r} == {second!r}"))

    def setUp(self) -> None:
        pass

    def tearDown(self) -> None:
        pass


class _BoundAssertion:
    _receiver: TestCase

    def __init__(self, case: TestCase) -> None:
        self._receiver = case


class _BoundEqual(_BoundAssertion):
    pass


class _BoundListEqual(_BoundAssertion):
    pass


class _BoundRaises(_BoundAssertion):
    pass


def _raise_call_type_error(message: str) -> None:
    raise TypeError(message)


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
    failures: list[tuple[TestCase, str]]
    errors: list[tuple[TestCase, str]]
    _verbosity: int

    def __init__(self) -> None:
        self.testsRun = 0
        self.shouldStop = False
        self.failfast = False
        self.skipped = []
        self.unexpectedSuccesses = []
        self.failures = []
        self.errors = []
        self._verbosity = -1

    def wasSuccessful(self) -> bool:
        return len(self.failures) == 0 and len(self.errors) == 0 and len(self.unexpectedSuccesses) == 0

    def startTestRun(self) -> None:
        pass

    def stopTestRun(self) -> None:
        pass

    def startTest(self, test: TestCase) -> None:
        self.testsRun += 1
        if self._verbosity > 1:
            sys.stderr.write(test._description() + " ... ")

    def stopTest(self, test: TestCase) -> None:
        pass

    def addSuccess(self, test: TestCase) -> None:
        self._status("ok", ".")

    def _status(self, full: str, short: str) -> None:
        if self._verbosity > 1:
            sys.stderr.write(full + "\n")
        elif self._verbosity == 1:
            sys.stderr.write(short)

    def _failure(self, test: TestCase, reason: str) -> None:
        if self.failfast:
            self.stop()
        self.failures.append((test, reason))
        self._status("FAIL", "F")

    def _error(self, test: TestCase, reason: str) -> None:
        if self.failfast:
            self.stop()
        self.errors.append((test, reason))
        self._status("ERROR", "E")

    def addSkip(self, test: TestCase, reason: str) -> None:
        self.skipped.append((test, reason))
        self._status("skipped " + repr(reason), "s")

    def addUnexpectedSuccess(self, test: TestCase) -> None:
        if self.failfast:
            self.stop()
        self.unexpectedSuccesses.append(test)

    def stop(self) -> None:
        self.shouldStop = True


def _members(tests: list[_SuiteItem] | None) -> list[_SuiteItem | None]:
    contents: list[_SuiteItem | None] = []
    if tests is not None:
        for test in tests:
            contents.append(test)
    return contents


class TestSuite(_SuiteItem):
    _tests: list[_SuiteItem | None]
    _removed_tests: int
    _cleanup: bool

    def __init__(self, tests: list[_SuiteItem] | None = None) -> None:
        self._tests = _members(tests)
        self._removed_tests = 0
        self._cleanup = True

    def _as_suite(self) -> TestSuite | None:
        return self

    def addTest(self, test: _SuiteItem) -> None:
        self._tests.append(test)

    def addTests(self, tests: list[_SuiteItem]) -> None:
        for test in tests:
            self.addTest(test)

    def countTestCases(self) -> int:
        count = 0
        groups: list[TestSuite] = [self]
        while len(groups):
            group = groups.pop()
            count += group._removed_tests
            for test in group._tests:
                if test is not None:
                    child = test._as_suite()
                    if child is not None:
                        groups.append(child)
                    else:
                        count += test._leaf_count()
        return count

    def _run_item(self, result: TestResult) -> None:
        self.run(result)

    def _missing_result(self) -> TestResult:
        raise TypeError("TestSuite.run() missing 1 required positional argument: 'result'")

    def run(self, result: TestResult | None = None) -> TestResult:
        if result is None:
            raise AttributeError("'NoneType' object has no attribute '_testRunEntered' and no __dict__ for setting new attributes")
        groups: list[TestSuite] = [self]
        positions: list[int] = [0]
        while len(groups):
            group = groups[-1]
            index = positions[-1]
            if index >= len(group._tests) or result.shouldStop:
                groups.pop()
                positions.pop()
                if len(groups):
                    parent = groups[-1]
                    if parent._cleanup:
                        parent._removed_tests += group.countTestCases()
                        parent._tests[positions[-1] - 1] = None
                continue
            test = group._tests[index]
            if test is None:
                raise TypeError("'NoneType' object is not callable")
            positions[-1] = index + 1
            child = test._as_suite()
            if child is not None:
                groups.append(child)
                positions.append(0)
            else:
                test._run_leaf(result)
                if group._cleanup:
                    group._removed_tests += test._leaf_count()
                    group._tests[index] = None
        return result


class TestLoader:
    testMethodPrefix: str

    def __init__(self) -> None:
        self.testMethodPrefix = "test"

    def _check_prefix(self) -> None:
        if self.testMethodPrefix != "test":
            raise NotImplementedError("native module discovery currently requires testMethodPrefix='test'")


defaultTestLoader = TestLoader()


class TextTestRunner:
    verbosity: int
    failfast: bool

    def __init__(self, *, verbosity: int = 1, failfast: bool = False) -> None:
        self.verbosity = verbosity
        self.failfast = failfast

    def run(self, test: _SuiteItem) -> TestResult:
        result = TestResult()
        result._verbosity = self.verbosity
        result.failfast = self.failfast
        start = time.time()
        result.startTestRun()
        try:
            test._run_item(result)
        finally:
            result.stopTestRun()
        elapsed = time.time() - start
        if self.verbosity > 0:
            sys.stderr.write("\n")
        for entry in result.errors:
            sys.stderr.write("=" * 70 + "\nERROR: " + entry[0]._description() + "\n" + "-" * 70 + "\n" + entry[1] + "\n\n")
        for entry in result.failures:
            sys.stderr.write("=" * 70 + "\nFAIL: " + entry[0]._description() + "\n" + "-" * 70 + "\n" + entry[1] + "\n\n")
        suffix = "" if result.testsRun == 1 else "s"
        sys.stderr.write("-" * 70 + f"\nRan {result.testsRun} test{suffix} in {elapsed:.3f}s\n\n")
        details: list[str] = []
        if len(result.failures):
            details.append("failures=" + str(len(result.failures)))
        if len(result.errors):
            details.append("errors=" + str(len(result.errors)))
        if len(result.skipped):
            details.append("skipped=" + str(len(result.skipped)))
        message = "OK" if result.wasSuccessful() else "FAILED"
        if result.wasSuccessful() and result.testsRun == 0 and not len(result.skipped):
            message = "NO TESTS RAN"
        if len(details):
            message = message + " (" + ", ".join(details) + ")"
        sys.stderr.write(message + "\n")
        return result


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
