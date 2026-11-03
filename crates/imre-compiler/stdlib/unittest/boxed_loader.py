# Runtime-loaded modules expose actual bindings and classes, including aliases
# and descriptors. The fixed-layout frontend continues to use static metadata.
class FunctionTestCase(TestCase):
    """A test case that wraps a test function.

    This is useful for slipping pre-existing test functions into the
    unittest framework. Optionally, set-up and tidy-up functions can be
    supplied. As with TestCase, the tidy-up ('tearDown') function will
    always be called if the set-up ('setUp') function ran successfully.
    """

    def __init__(self, testFunc, setUp=None, tearDown=None, description=None):
        super(FunctionTestCase, self).__init__()
        self._setUpFunc = setUp
        self._tearDownFunc = tearDown
        self._testFunc = testFunc
        self._description = description

    def setUp(self):
        if self._setUpFunc is not None:
            self._setUpFunc()

    def tearDown(self):
        if self._tearDownFunc is not None:
            self._tearDownFunc()

    def runTest(self):
        self._testFunc()

    def id(self):
        return self._testFunc.__name__

    def __eq__(self, other):
        if not isinstance(other, self.__class__):
            return NotImplemented

        return self._setUpFunc == other._setUpFunc and \
               self._tearDownFunc == other._tearDownFunc and \
               self._testFunc == other._testFunc and \
               self._description == other._description

    def __hash__(self):
        return hash((type(self), self._setUpFunc, self._tearDownFunc,
                     self._testFunc, self._description))

    def __str__(self):
        return "%s (%s)" % (self.__class__.__module__ + "." + self.__class__.__qualname__,
                            self._testFunc.__name__)

    def __repr__(self):
        return "<%s tec=%s>" % (self.__class__.__module__ + "." + self.__class__.__qualname__,
                                     self._testFunc)

    def shortDescription(self):
        if self._description is not None:
            return self._description
        doc = self._testFunc.__doc__
        return doc and doc.split("\n")[0].strip() or None


def _boxedThreeWayCompare(left, right):
    return (left > right) - (left < right)


def _boxedLoaderInit(self):
    self.errors = []
    self._loading_packages = set()


def getTestCaseNames(self, testCaseClass):
    names = []
    for name in dir(testCaseClass):
        if not name.startswith(self.testMethodPrefix):
            continue
        if not callable(getattr(testCaseClass, name)):
            continue
        fullname = testCaseClass.__module__ + "." + testCaseClass.__qualname__ + "." + name
        patterns = self.testNamePatterns
        if patterns is None or any(fnmatch.fnmatchcase(fullname, pattern) for pattern in patterns):
            names.append(name)
    # dir() already supplies the default lexical order.
    comparison = self.sortTestMethodsUsing
    if comparison and comparison is not _boxedThreeWayCompare:
        from functools import cmp_to_key
        names.sort(key=cmp_to_key(comparison))
    return names


def loadTestsFromTestCase(self, testCaseClass):
    if issubclass(testCaseClass, TestSuite):
        raise TypeError("Test cases should not be derived from TestSuite. Maybe you meant to derive from TestCase?")
    if testCaseClass is TestCase or testCaseClass is FunctionTestCase:
        return self.suiteClass(())
    names = self.getTestCaseNames(testCaseClass)
    if not names and hasattr(testCaseClass, "runTest"):
        names = ["runTest"]
    if names and issubclass(testCaseClass, TestCase):
        _boxedCheckFixtures(testCaseClass)
    return self.suiteClass(map(testCaseClass, names))


def loadTestsFromModule(self, module, *, pattern=None):
    groups = []
    for name in dir(module):
        binding = getattr(module, name)
        if (isinstance(binding, type) and issubclass(binding, TestCase)
                and binding is not TestCase and binding is not FunctionTestCase):
            groups.append(self.loadTestsFromTestCase(binding))
    hook = getattr(module, "load_tests", None)
    suite = self.suiteClass(groups)
    if hook is not None:
        try:
            return hook(self, suite, pattern)
        except Exception as error:
            import traceback
            message = "Failed to call load_tests:\n" + traceback.format_exc()
            failure = self.suiteClass((_FailedTest(module.__name__, error),))
            self.errors.append(message)
            return failure
    return suite


def _boxedFailedInit(self, module_name, exception):
    self._module_name = module_name
    self._exception = exception
    TestCase.__init__(self, module_name)


def _boxedFailedMethod(self, name):
    if name == self._testMethodName:
        return self.runTest
    raise AttributeError(name)


def _boxedFailedDescription(self):
    return self._testMethodName + " (" + self.id() + ")"


_FailedTest._testMethodName = None
_FailedTest.__init__ = _boxedFailedInit
_FailedTest.__getattr__ = _boxedFailedMethod
_FailedTest.__str__ = _boxedFailedDescription
TestLoader.__init__ = _boxedLoaderInit
TestLoader.testMethodPrefix = "test"
TestLoader.testNamePatterns = None
TestLoader._top_level_dir = None
TestLoader.sortTestMethodsUsing = staticmethod(_boxedThreeWayCompare)
TestLoader.getTestCaseNames = getTestCaseNames
TestLoader.loadTestsFromTestCase = loadTestsFromTestCase
TestLoader.loadTestsFromModule = loadTestsFromModule
del getTestCaseNames, loadTestsFromTestCase, loadTestsFromModule
defaultTestLoader = TestLoader()


import os


# From CPython 3.14.5 Lib/unittest/main.py. See ../docstrings/LICENSE.
def _convert_name(name):
    # on Linux / Mac OS X 'foo.PY' is not importable, but on
    # Windows it is. Simpler to do a case insensitive match
    # a better check would be to check that the name is a
    # valid Python module name.
    if os.path.isfile(name) and name.lower().endswith('.py'):
        if os.path.isabs(name):
            rel_path = os.path.relpath(name, os.getcwd())
            if os.path.isabs(rel_path) or rel_path.startswith(os.pardir):
                return name
            name = rel_path
        # on Windows both '\' and '/' are used as path
        # separators. Better to replace both than rely on os.path.sep
        return os.path.normpath(name)[:-3].replace('\\', '.').replace('/', '.')
    return name


def _main_option(arg):
    # argparse recognizes a known option before treating strings with spaces as names.
    if arg[:2] in ("-k", "-v", "-q", "-f"):
        return True
    if "=" in arg and arg.partition("=")[0] in ("--verbose", "--quiet", "--failfast"):
        return True
    return (arg.startswith("-") and arg != "-" and " " not in arg
            and not re.match(r"-\.?\d", arg))


def main(module="__main__"):
    runner = _MainRunner()
    names = []
    patterns = []
    extras = []
    names_finished = False
    args = iter(sys.argv[1:])
    for arg in args:
        if arg == "--":
            if names_finished:
                extras.append(arg)
                extras.extend(args)
            else:
                names.extend(args)
            break
        if not _main_option(arg):
            (extras if names_finished else names).append(arg)
            continue
        # The positional `tests` argument is a single group. Options may precede or follow it.
        names_finished = bool(names)
        if arg.startswith("--"):
            runner._setOption(next((flag for flag in ("--verbose", "--quiet", "--failfast")
                                    if flag.startswith(arg)), arg))
            continue
        for i, flag in enumerate(arg[1:], 2):
            if flag != "k":
                runner._setOption("-" + flag)
                continue
            pattern = arg[i:]
            if pattern:
                pattern = pattern.removeprefix("=")
            else:
                pattern = next(args, None)
                if pattern is None or _main_option(pattern):
                    print("argument -k: expected one argument", file=sys.stderr)
                    sys.exit(2)
            patterns.append(pattern if "*" in pattern else "*" + pattern + "*")
            break
    if extras:
        print("unrecognized arguments: " + " ".join(extras), file=sys.stderr)
        sys.exit(2)
    if patterns:
        defaultTestLoader.testNamePatterns = patterns
    if isinstance(module, str):
        module = __import__(module, fromlist=["__name__"])
    if names:
        suite = defaultTestLoader.loadTestsFromNames([_convert_name(name) for name in names], module)
    elif module is None:
        suite = defaultTestLoader.discover(".")
    else:
        suite = defaultTestLoader.loadTestsFromModule(module)
    result = TextTestRunner(verbosity=runner.verbose, failfast=runner.failfast).run(suite)
    if not result.wasSuccessful():
        sys.exit(1)
    if not result.testsRun and not result.skipped:
        sys.exit(5)
    sys.exit(0)


import unittest.case as case
from unittest.case import addModuleCleanup, doModuleCleanups, enterModuleContext, expectedFailure

TestCase._addExpectedFailure = case._addExpectedFailure
TestCase._addUnexpectedSuccess = case._addUnexpectedSuccess

from unittest import loader, signals, util
from unittest.signals import registerResult, removeResult
