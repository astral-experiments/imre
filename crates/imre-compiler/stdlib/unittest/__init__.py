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
