"""Discover interactive examples and compare their output in native programs."""

from collections.abc import Iterator
from typing import Callable
from _imre_doctest import exception_name, execute
import unittest

DONT_ACCEPT_TRUE_FOR_1 = 1
DONT_ACCEPT_BLANKLINE = 2
NORMALIZE_WHITESPACE = 4
ELLIPSIS = 8
SKIP = 16
IGNORE_EXCEPTION_DETAIL = 32
COMPARISON_FLAGS = 63
REPORT_UDIFF = 64
REPORT_CDIFF = 128
REPORT_NDIFF = 256
REPORT_ONLY_FIRST_FAILURE = 512
FAIL_FAST = 1024
REPORTING_FLAGS = 1984


def _example_exception(msg: str | None) -> str | None:
    if msg is not None and not msg.endswith("\n"):
        return msg + "\n"
    return msg


def _example_options(options: dict[int, bool] | None) -> dict[int, bool]:
    if options is None:
        return {}
    return options


class Example:
    source: str
    want: str
    exc_msg: str | None
    lineno: int
    indent: int
    options: dict[int, bool]

    def __init__(self, source: str, want: str, exc_msg: str | None = None,
                 lineno: int = 0, indent: int = 0,
                 options: dict[int, bool] | None = None) -> None:
        self.source = source if source.endswith("\n") else source + "\n"
        self.want = want if not want or want.endswith("\n") else want + "\n"
        self.exc_msg = _example_exception(exc_msg)
        self.lineno = lineno
        self.indent = indent
        self.options = _example_options(options)


def _spaces(line: str) -> int:
    i = 0
    while i < len(line) and line[i] == " ":
        i += 1
    return i


def _expand_tabs(line: str) -> str:
    if line.find("\t") < 0:
        return line
    result = ""
    for char in line:
        if char == "\t":
            result += " " * (8 - len(result) % 8)
        else:
            result += char
    return result


def _prompt(line: str, indent: int, name: str, lineno: int) -> str:
    if len(line) > indent + 3 and line[indent + 3] != " ":
        raise ValueError(f"line {lineno + 1} of the docstring for {name} lacks blank after {line[indent:indent + 3]}: {line!r}")
    return line[indent + 4:]


def _exception(want: str) -> str | None:
    lines = want.split("\n")
    header = lines[0].rstrip()
    if header != "Traceback (most recent call last):" and header != "Traceback (innermost last):":
        return None
    for i in range(1, len(lines)):
        line = lines[i]
        if line and "_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".find(line[0]) >= 0:
            return "\n".join(lines[i:])
    return None


def _flags(source: str, name: str, lineno: int, options: dict[int, bool]) -> None:
    names = ["DONT_ACCEPT_TRUE_FOR_1", "DONT_ACCEPT_BLANKLINE", "NORMALIZE_WHITESPACE",
             "ELLIPSIS", "SKIP", "IGNORE_EXCEPTION_DETAIL", "REPORT_UDIFF", "REPORT_CDIFF",
             "REPORT_NDIFF", "REPORT_ONLY_FIRST_FAILURE", "FAIL_FAST"]
    for line in source.split("\n"):
        pos = line.find("#")
        while pos >= 0:
            comment = line[pos + 1:].lstrip()
            if comment.startswith("doctest:") and "'" not in comment and '"' not in comment:
                for option in comment[8:].replace(",", " ").split():
                    n = 0
                    while n < len(names) and names[n] != option[1:]:
                        n += 1
                    if n == len(names) or (option[0] != "+" and option[0] != "-"):
                        raise ValueError(f"line {lineno + 1} of the doctest for {name} has an invalid option: {option!r}")
                    options[1 << n] = option[0] == "+"
                break
            rest = line[pos + 1:].find("#")
            pos = -1 if rest < 0 else pos + 1 + rest


class DocTest:
    name: str
    docstring: str
    examples: list[Example]
    globs: dict[str, object]
    filename: str | None
    lineno: int | None

    def __init__(self, name: str, docstring: str, examples: list[Example]) -> None:
        self.name = name
        self.docstring = docstring
        self.examples = examples
        self.globs = {}
        self.filename = None
        self.lineno = None


class DocTestParser:
    def get_doctest(self, string: str, globs: dict[str, object], name: str,
                    filename: str | None, lineno: int | None) -> DocTest:
        test = DocTest(name, string, self.get_examples(string, name))
        test.globs = globs.copy()
        test.filename = filename
        test.lineno = lineno
        return test

    def get_examples(self, string: str, name: str = "<string>") -> list[Example]:
        lines = [_expand_tabs(line) for line in string.split("\n")]
        examples: list[Example] = []
        i = 0
        while i < len(lines):
            line = lines[i]
            indent = _spaces(line)
            if not line[indent:].startswith(">>>"):
                i += 1
                continue
            start = i
            source = _prompt(line, indent, name, i)
            i += 1
            while i < len(lines) and lines[i][_spaces(lines[i]):].startswith("..."):
                line = lines[i]
                if _spaces(line) != indent:
                    raise ValueError(f"line {i + 1} of the docstring for {name} has inconsistent leading whitespace: {line!r}")
                source += "\n" + _prompt(line, indent, name, i)
                i += 1
            want = ""
            while i < len(lines):
                line = lines[i]
                spaces = _spaces(line)
                if spaces == len(line) or line[spaces:].startswith(">>>"):
                    break
                if spaces < indent:
                    raise ValueError(f"line {i + 1} of the docstring for {name} has inconsistent leading whitespace: {line!r}")
                want += line[indent:] + "\n"
                i += 1
            example = Example(source, want, _exception(want), start, indent)
            _flags(source, name, start, example.options)
            stripped = source.strip()
            if stripped and not (stripped.startswith("#") and stripped.find("\n") < 0):
                examples.append(example)
            elif example.options:
                raise ValueError(f"line {start} of the doctest for {name} has an option directive on a line with no example: {source!r}")
        return examples


def _ellipsis(want: str, got: str) -> bool:
    parts = want.split("...")
    if len(parts) == 1:
        return want == got
    first = parts[0]
    last = parts[len(parts) - 1]
    if not got.startswith(first) or not got.endswith(last):
        return False
    start = len(first)
    end = len(got) - len(last)
    if start > end:
        return False
    for i in range(1, len(parts) - 1):
        part = parts[i]
        offset = got[start:end].find(part)
        if offset < 0:
            return False
        start += offset + len(part)
    return True


class OutputChecker:
    def _toAscii(self, s: str) -> str:
        """Convert string to hex-escaped ASCII string."""
        return str(s.encode("ASCII", "backslashreplace"), "ASCII")

    def check_output(self, want: str, got: str, optionflags: int) -> bool:
        got = self._toAscii(got)
        want = self._toAscii(want)
        if want == got:
            return True
        if not optionflags & 1 and ((want == "1\n" and got == "True\n") or
                                    (want == "0\n" and got == "False\n")):
            return True
        if not optionflags & 2:
            want = "\n".join(["" if line.rstrip() == "<BLANKLINE>" else line
                              for line in want.split("\n")])
            got = "\n".join(["" if not line.strip() else line for line in got.split("\n")])
        if optionflags & 4:
            want = " ".join(want.split())
            got = " ".join(got.split())
        if optionflags & 8:
            return _ellipsis(want, got)
        return want == got


class DocTestFailure(Exception):
    """A DocTest example has failed in debugging mode.

    The exception instance has variables:

    - test: the DocTest object being run

    - example: the Example object that failed

    - got: the actual output
    """
    def __init__(self, test, example, got):
        self.test = test
        self.example = example
        self.got = got

    def __str__(self):
        return str(self.test)


class UnexpectedException(Exception):
    """A DocTest example has encountered an unexpected exception

    The exception instance has variables:

    - test: the DocTest object being run

    - example: the Example object that failed

    - exc_info: the exception info
    """
    def __init__(self, test, example, exc_info):
        self.test = test
        self.example = example
        self.exc_info = exc_info

    def __str__(self):
        return str(self.test)


def _optionflags(example: Example, flags: int = 0) -> int:
    for flag in example.options:
        if example.options[flag]:
            flags |= flag
        else:
            flags &= ~flag
    return flags


def _example_location(test: DocTest, example: Example) -> str:
    if test.filename:
        base = test.lineno
        line = "?" if base is None else str(base + example.lineno + 1)
        return f'File "{test.filename}", line {line}, in {test.name}'
    return f"Line {example.lineno + 1}, in {test.name}"


def _check_example(test: DocTest, example: Example, got: str, error: str,
                   failures: list[str], debug: bool) -> None:
    _check_output(test, example, got, error,
                  failures, debug, OutputChecker(), _optionflags(example))


def _strip_exception_details(message: str) -> str:
    return message.partition("\n")[0].partition(":")[0].split(".")[-1]


def _check_output(test: DocTest, example: Example, got: str, error: str,
                  failures: list[str], debug: bool, checker: OutputChecker, flags: int,
                  quiet: bool = False, exc_info: object = None) -> bool:
    expected = example.exc_msg
    if error:
        if expected is not None:
            if checker.check_output(expected, error, flags):
                return True
            if flags & IGNORE_EXCEPTION_DETAIL and checker.check_output(
                _strip_exception_details(expected), _strip_exception_details(error), flags
            ):
                return True
    elif checker.check_output(example.want, got, flags):
        return True
    if quiet:
        return False
    if debug:
        if error and expected is None:
            raise UnexpectedException(test, example, exc_info)
        if exc_info is not None:
            from traceback import format_exception
            got += "".join(format_exception(*exc_info))
        raise DocTestFailure(test, example, got)
    message = f"{_example_location(test, example)}\nFailed example:\n{example.source}"
    if error and expected is None:
        message += "Exception raised:\n" + error
    else:
        message += f"Expected:\n{example.want}Got:\n{error or got}"
    failures.append(message)
    return False


def _exception_line(error: BaseException) -> str:
    message = str(error)
    name = exception_name(error)
    if message:
        return name + ": " + message + "\n"
    return name + "\n"


def _call_hook(hook: Callable[[DocTest], None] | None, test: DocTest) -> None:
    if hook is not None:
        hook(test)


class _DocTestCase(unittest.TestCase):
    _dt_setUp = None
    _dt_tearDown = None
    _dt_test: DocTest
    _dt_globs: dict[str, object]

    def __init__(self, test: DocTest) -> None:
        super().__init__()
        self._dt_test = test
        self._dt_globs = {}

    def setUp(self) -> None:
        test = self._dt_test
        self._dt_globs = test.globs.copy()
        _call_hook(self._dt_setUp, test)

    def tearDown(self) -> None:
        self._tear_down(False)

    def _tear_down(self, restore_on_error: bool) -> None:
        test = self._dt_test
        restore = restore_on_error
        try:
            _call_hook(self._dt_tearDown, test)
            restore = True
        finally:
            if restore:
                test.globs.clear()
                test.globs.update(self._dt_globs)

    def id(self) -> str:
        return self._dt_test.name

    def countTestCases(self) -> int:
        return 1

    def runTest(self) -> None:
        _execute_case(self, False)

    def debug(self) -> None:
        self.setUp()
        _execute_case(self, True)
        self.tearDown()
