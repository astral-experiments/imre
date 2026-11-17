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
        lines = string.expandtabs().split("\n")
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


class DocTestCase(unittest.TestCase):
    _dt_setUp = None
    _dt_tearDown = None
    _dt_test: DocTest
    _dt_globs: dict[str, object]
    _dt_optionflags: int
    _dt_checker: OutputChecker | None

    def __init__(self, test: DocTest, optionflags: int = 0,
                 setUp: Callable[[DocTest], None] | None = None,
                 tearDown: Callable[[DocTest], None] | None = None,
                 checker: OutputChecker | None = None) -> None:
        super().__init__()
        self._dt_optionflags = optionflags
        self._dt_checker = checker
        self._dt_test = test
        self._dt_globs = {}
        self._dt_setUp = setUp
        self._dt_tearDown = tearDown

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
        execute(self._dt_test, False, self._dt_optionflags, self._dt_checker)

    def debug(self) -> None:
        self.setUp()
        execute(self._dt_test, True, self._dt_optionflags, self._dt_checker)
        self.tearDown()


def _suite_from_tests(tests: list[DocTest], options: dict[str, object]) -> _DocTestSuite:
    suite = _DocTestSuite()
    for test in tests:
        if len(test.examples) == 0:
            continue
        suite.addTest(DocTestCase(test, **options))
    return suite

# The boxed importer discovers docs from the live module instead of a fixed module graph.
from collections import namedtuple
import os
import sys
import types


# From CPython 3.14.5, Lib/doctest.py (PSF-2.0).
class TestResults(namedtuple('TestResults', 'failed attempted')):
    def __new__(cls, failed, attempted, *, skipped=0):
        results = super().__new__(cls, failed, attempted)
        results.skipped = skipped
        return results

    def __repr__(self):
        if self.skipped:
            return (f'TestResults(failed={self.failed}, '
                    f'attempted={self.attempted}, '
                    f'skipped={self.skipped})')
        else:
            # Leave the repr() unchanged for backward compatibility
            # if skipped is zero
            return super().__repr__()


def _from_module(item, owner):
    name = getattr(item, "__module__", None)
    module = sys.modules.get(name)
    if module is not None:
        return module is owner
    if isinstance(item, types.FunctionType):
        return item.__globals__ is owner.__dict__
    return name == owner.__name__


def _boxed_docs(obj, name, owner, found, seen):
    key = id(obj)
    if key in seen:
        return
    seen.add(key)
    doc = obj if isinstance(obj, str) else getattr(obj, "__doc__", None)
    if isinstance(doc, str) and doc:
        found[name] = (obj, doc)
    is_module = isinstance(obj, types.ModuleType)
    if not is_module and not isinstance(obj, type):
        return
    routines = (types.FunctionType, types.BuiltinFunctionType, types.MethodType)
    for attr, item in obj.__dict__.items():
        if not is_module and isinstance(item, (staticmethod, classmethod)):
            item = item.__func__
        if not is_module and isinstance(item, property):
            _boxed_docs(item, name + "." + attr, owner, found, seen)
        elif isinstance(item, type) or isinstance(item, routines):
            if _from_module(item, owner):
                _boxed_docs(item, name + "." + attr, owner, found, seen)
    if is_module:
        for attr, item in getattr(obj, "__test__", {}).items():
            if not isinstance(attr, str):
                raise ValueError("DocTestFinder.find: __test__ keys must be strings: %r" % (type(attr),))
            if not isinstance(item, (str, types.ModuleType, type)) and not isinstance(item, routines):
                raise ValueError("DocTestFinder.find: __test__ values must be strings, functions, methods, classes, or modules: %r" % (type(item),))
            _boxed_docs(item, name + ".__test__." + attr, owner, found, seen)


def _docstring_lineno(obj, lines, filename):
    if isinstance(obj, types.ModuleType):
        start = 0
    elif isinstance(obj, type):
        if not lines:
            return None
        first = vars(obj).get("__firstlineno__")
        if first is None:
            return None
        start = first - 1
    else:
        if isinstance(obj, property):
            obj = obj.fget
        if hasattr(obj, "__wrapped__"):
            from inspect import unwrap
            obj = unwrap(obj)
        code = getattr(obj, "__code__", None)
        if code is None or (lines and code.co_filename != filename):
            return None
        start = code.co_firstlineno - 1
    if not lines:
        return start + 1
    # DocTestFinder counts from the opening quote, including on a definition line.
    import re
    for index in range(start, len(lines)):
        if re.match(r"(^|.*:)\s*\w*['\"]", lines[index]):
            return index
    return None


class _DocTestSuite(unittest.TestSuite):
    def _removeTestAtIndex(self, index):
        pass


def DocTestSuite(module=None, globs=None, extraglobs=None, **options):
    if "test_finder" in options:
        raise TypeError("DocTestSuite() got an unexpected keyword argument 'test_finder'")
    from _imre_importlib import bind_doctest
    module = _normalize_module(module)
    namespace = (module.__dict__ if globs is None else globs).copy()
    if extraglobs is not None:
        namespace.update(extraglobs)
    namespace.setdefault("__name__", "__main__")
    found = {}
    _boxed_docs(module, module.__name__, module, found, set())
    parser = DocTestParser()
    tests = []
    filename = getattr(module, "__file__", None) or module.__name__
    if filename.endswith(".pyc"):
        filename = filename[:-1]
    lines = None if filename.endswith((".py", ".pyw")) else []
    for name in sorted(found):
        obj, doc = found[name]
        test = parser.get_doctest(doc, namespace, name, filename, None)
        if test.examples:
            bind_doctest(test, module)
            if lines is None:
                import linecache
                lines = linecache.getlines(filename, module.__dict__)
            test.lineno = _docstring_lineno(obj, lines, filename)
        tests.append(test)
    if not tests and sys.flags.optimize >= 2:
        # Skip doctests when running with -O2
        suite = _DocTestSuite()
        suite.addTest(SkipDocTestCase(module))
        return suite

    return _suite_from_tests(tests, options)


# Live cases store both hooks on the instance, including None.
del DocTestCase._dt_setUp, DocTestCase._dt_tearDown


class SkipDocTestCase(DocTestCase):
    def __init__(self, module):
        self.module = module
        DocTestCase.__init__(self, None)

    def setUp(self):
        self.skipTest("DocTestSuite will not work with -O2 and above")

    def test_skip(self):
        pass

    def shortDescription(self):
        return "Skipping tests from %s" % self.module.__name__

    __str__ = shortDescription



class DocFileCase(DocTestCase):
    def id(self):
        return self._dt_test.name.replace(".", "_")

    def __repr__(self):
        return self._dt_test.filename

    __str__ = __repr__

    def shortDescription(self):
        return "Doctest: " + self._dt_test.name

    def tearDown(self):
        # File cases restore globals before unittest cleanups even if their hook raises.
        self._tear_down(True)

    def runTest(self):
        test = self._dt_test
        try:
            super().runTest()
        except AssertionError as exc:
            raise self.failureException(
                f'Failed doctest test for {test.name}\n  File "{test.filename}", line 0\n\n{exc}'
            ) from None


def _normalize_module(module):
    if module is None:
        return sys.modules[sys._getframemodulename(2)]
    if isinstance(module, str):
        return __import__(module, fromlist=["*"])
    if not isinstance(module, types.ModuleType):
        raise TypeError("Expected a module, string, or None")
    return module


def _docfile_path(package, path):
    if path.startswith("/"):
        raise ValueError("Module-relative files may not have absolute paths")
    if getattr(package, "__file__", None) is not None:
        return os.path.join(os.path.dirname(package.__file__), path)
    if package.__name__ == "__main__":
        directory = os.path.dirname(sys.argv[0]) if sys.argv and sys.argv[0] else os.curdir
        return os.path.join(directory, path)
    for directory in getattr(package, "__path__", ()):
        filename = os.path.join(directory, path)
        if os.path.exists(filename):
            return filename
    raise ValueError(f"Can't resolve paths relative to the module {package.__name__!r} (it has no __file__)")


def DocFileTest(path, module_relative=True, package=None, globs=None,
                parser=DocTestParser(), encoding=None, **options):
    get_data = None
    if module_relative:
        package = _normalize_module(package)
        path = _docfile_path(package, path)
        loader = getattr(package, "__loader__", None)
        if loader is None:
            loader = getattr(getattr(package, "__spec__", None), "loader", None)
        get_data = getattr(loader, "get_data", None)
    elif package is not None:
        raise ValueError("Package may only be specified for module-relative paths.")
    if get_data is None:
        with open(path, encoding=encoding or "utf-8") as stream:
            text = stream.read()
    else:
        text = get_data(path).decode(encoding or "utf-8").replace("\r\n", "\n").replace("\r", "\n")
    namespace = {} if globs is None else globs.copy()
    namespace.setdefault("__file__", path)
    test = parser.get_doctest(text, namespace, os.path.basename(path), path, 0)
    return DocFileCase(test, **options)


def DocFileSuite(*paths, **options):
    if options.get("module_relative", True):
        options["package"] = _normalize_module(options.get("package"))
    suite = _DocTestSuite()
    for path in paths:
        suite.addTest(DocFileTest(path, **options))
    return suite
