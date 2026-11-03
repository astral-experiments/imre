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


import unittest.case as case
from unittest.case import addModuleCleanup, doModuleCleanups, enterModuleContext, expectedFailure

TestCase._addExpectedFailure = case._addExpectedFailure
TestCase._addUnexpectedSuccess = case._addUnexpectedSuccess

from unittest import loader, signals, util
from unittest.main import TestProgram, main
from unittest.signals import registerResult, removeResult


# Lazy import of IsolatedAsyncioTestCase from .async_case
# It imports asyncio, which is relatively heavy, but most tests
# do not need it.

def __dir__():
    return globals().keys() | {'IsolatedAsyncioTestCase'}

def __getattr__(name):
    if name == 'IsolatedAsyncioTestCase':
        global IsolatedAsyncioTestCase
        from .async_case import IsolatedAsyncioTestCase
        return IsolatedAsyncioTestCase
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
