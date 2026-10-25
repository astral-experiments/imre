"""Native unittest operations for Imre's fixed-layout test cases."""


import difflib
from types import ModuleType
from typing import ClassVar


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


class TestCase:
    failureException: ClassVar[type[Exception]] = AssertionError
    longMessage: bool
    maxDiff: int | None
    _diffThreshold: int

    def __init__(self) -> None:
        self.longMessage = True
        self.maxDiff = 640
        self._diffThreshold = 65536

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
            raise self.failureException(self._formatMessage(msg, f"{first!r} != {second!r}"))

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
        else:
            raise self.failureException(self._formatMessage(msg, standard))
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
            raise self.failureException(self._formatMessage(msg, f"{first!r} == {second!r}"))

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
