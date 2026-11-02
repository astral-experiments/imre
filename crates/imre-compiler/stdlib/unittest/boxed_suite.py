# The boxed runtime can validate and call bound methods on the actual instance.
def _validateCaseMethod(case, methodName):
    try:
        method = getattr(case, methodName)
    except AttributeError:
        if methodName != "runTest":
            raise ValueError("no such test method in " + str(type(case)) + ": " + methodName)
        case._testMethodDoc = "No test"
    else:
        case._testMethodDoc = getattr(method, "__doc__", None)
    case._cleanups = []


def _boxedCheckFixtures(cls):
    if not issubclass(cls, TestCase):
        for name in ("addClassCleanup", "enterClassContext", "doClassCleanups"):
            if getattr(cls, name, None) is not None:
                raise NotImplementedError("native unittest does not yet support class cleanup hooks: " + name)
        return
    for name in ("run", "__call__"):
        for owner in cls.__mro__:
            if owner is object:
                break
            if name in owner.__dict__:
                if owner is not TestCase:
                    raise NotImplementedError("native unittest does not yet support custom result hooks: "
                                              + owner.__qualname__ + "." + name)
                break


def _runRegistered(result, test, name, stopTestRun=None):
    result.startTest(test)
    try:
        _runBoxedCase(result, test)
    finally:
        result.stopTest(test)
        if stopTestRun is not None:
            stopTestRun()


def _runBoxedCase(result, test):
    cls = type(test)
    method = getattr(test, test._testMethodName)
    if getattr(cls, "__unittest_skip__", False) or getattr(method, "__unittest_skip__", False):
        reason = getattr(cls, "__unittest_skip_why__", "") or getattr(method, "__unittest_skip_why__", "")
        case._addSkip(result, test, reason)
        return
    expecting_failure = (getattr(test, "__unittest_expecting_failure__", False)
                         or getattr(method, "__unittest_expecting_failure__", False))
    outcome = case._Outcome(result)
    start_time = case.time.perf_counter()
    try:
        test._outcome = outcome
        with outcome.testPartExecutor(test):
            test._callSetUp()
        if outcome.success:
            outcome.expecting_failure = expecting_failure
            with outcome.testPartExecutor(test):
                test._callTestMethod(method)
            outcome.expecting_failure = False
            with outcome.testPartExecutor(test):
                test._callTearDown()
        test.doCleanups()
        test._addDuration(result, case.time.perf_counter() - start_time)
        if outcome.success:
            if not expecting_failure:
                result.addSuccess(test)
            elif outcome.expectedFailure is None:
                test._addUnexpectedSuccess(result)
            else:
                test._addExpectedFailure(result, outcome.expectedFailure)
    finally:
        outcome.expectedFailure = None
        test._outcome = None


def _boxedSubTestEnter(self):
    if self.entered:
        raise AttributeError("'_GeneratorContextManager' object has no attribute 'args'")
    self.entered = True
    outcome = getattr(self.case, "_outcome", None)
    if outcome is not None and outcome.result_supports_subtests:
        self.parent = self.case._subtest
        parents = () if self.parent is None else self.parent.params.maps
        params = case._OrderedChainMap(self.params, *parents)
        message = self.message if self.has_message else case._subtest_msg_sentinel
        subtest = case._SubTest(self.case, message, params)
        self.case._subtest = subtest
        self._executor = outcome.testPartExecutor(subtest, True)
        self._executor.__enter__()


def _boxedSubTestExit(self, typ, value, tb):
    outcome = getattr(self.case, "_outcome", None)
    if outcome is None or not outcome.result_supports_subtests:
        return False
    try:
        suppressed = self._executor.__exit__(typ, value, tb)
        if typ is not None and not suppressed:
            return False
        if outcome.expectedFailure or (not outcome.success and getattr(outcome.result, "failfast", False)):
            raise case._ShouldStop
        return suppressed
    finally:
        self.case._subtest = self.parent


TestCase._subtest = None
_SubTestContext.__enter__ = _boxedSubTestEnter
_SubTestContext.__exit__ = _boxedSubTestExit


TestResult._runRegistered = _runRegistered


def _caseEqual(self, other):
    if type(self) is not type(other):
        return NotImplemented
    return self._testMethodName == other._testMethodName


def _caseHash(self):
    return hash((type(self), self._testMethodName))


def _caseShortDescription(self):
    doc = self._testMethodDoc
    return doc.strip().split("\n")[0].strip() if doc else None


def _caseStr(self):
    method = self._testMethodName
    cls = type(self)
    return method + " (" + cls.__module__ + "." + cls.__qualname__ + "." + method + ")"


def _caseCallSetUp(self):
    self.setUp()


def _caseCallTestMethod(self, method):
    value = method()
    if value is not None:
        import types
        import warnings
        message = (f"It is deprecated to return a value that is not None "
                   f"from a test case ({method} returned {type(value).__name__!r})")
        if isinstance(value, getattr(types, "CoroutineType", ())):
            message += ". Maybe you forgot to use IsolatedAsyncioTestCase as the base class?"
        warnings.warn(message, DeprecationWarning, stacklevel=3)


def _caseCallTearDown(self):
    self.tearDown()


def _caseDebug(self):
    method = getattr(self, self._testMethodName)
    if getattr(type(self), "__unittest_skip__", False) or getattr(method, "__unittest_skip__", False):
        reason = getattr(type(self), "__unittest_skip_why__", "") or getattr(method, "__unittest_skip_why__", "")
        raise SkipTest(reason)
    self._callSetUp()
    self._callTestMethod(method)
    self._callTearDown()
    cleanups = getattr(self, "_cleanups", ())
    while cleanups:
        function, args, kwargs = cleanups.pop()
        self._callCleanup(function, *args, **kwargs)


def _caseRun(self, result=None):
    stopTestRun = None
    if result is None:
        result = self.defaultTestResult()
        startTestRun = getattr(result, "startTestRun", None)
        stopTestRun = getattr(result, "stopTestRun", None)
        if startTestRun is not None:
            startTestRun()
    _runRegistered(result, self, "", stopTestRun)
    return result


def _callTest(self, *args, **kwargs):
    return self.run(*args, **kwargs)


def _suiteInit(self, tests=()):
    self._tests = []
    self._removed_tests = 0
    self.addTests(tests)


def _suiteIter(self):
    return iter(self._tests)


def _suiteEqual(self, other):
    if not isinstance(other, type(self)):
        return NotImplemented
    return list(self) == list(other)


def _suiteCount(self):
    count = self._removed_tests
    for test in self:
        if test:
            count += test.countTestCases()
    return count


def _suiteAddTest(self, test):
    if not callable(test):
        raise TypeError(repr(test) + " is not callable")
    if isinstance(test, type) and issubclass(test, (TestCase, TestSuite)):
        raise TypeError("TestCases and TestSuites must be instantiated before passing them to addTest()")
    self._tests.append(test)


def _suiteAddTests(self, tests):
    if isinstance(tests, str):
        raise TypeError("tests must be an iterable of tests, not a string")
    for test in tests:
        self.addTest(test)


def _suiteRemoveTest(self, index):
    try:
        test = self._tests[index]
    except TypeError:
        return
    if hasattr(test, "countTestCases"):
        self._removed_tests += test.countTestCases()
    self._tests[index] = None


def _initCaseSubclass(cls, *args, **kwargs):
    cls._classSetupFailed = False
    cls._class_cleanups = []
    super(TestCase, cls).__init_subclass__(*args, **kwargs)


def _classCleanups(cls):
    # The bundled classes defined before these boxed methods also need their own stack.
    if "_class_cleanups" not in cls.__dict__:
        cls._class_cleanups = []
    return cls._class_cleanups


def _addClassCleanup(cls, function, /, *args, **kwargs):
    _classCleanups(cls).append((function, args, kwargs))


def _enterClassContext(cls, cm):
    return _enterCleanupContext(cm, cls.addClassCleanup)


def _doClassCleanups(cls):
    cls.tearDown_exceptions = []
    cleanups = _classCleanups(cls)
    while cleanups:
        function, args, kwargs = cleanups.pop()
        try:
            function(*args, **kwargs)
        except Exception:
            cls.tearDown_exceptions.append(sys.exc_info())


class _ErrorHolder:
    failureException = None

    def __init__(self, description):
        self.description = description

    def id(self):
        return self.description

    def __str__(self):
        return self.description

    def __repr__(self):
        return "<ErrorHolder description=" + repr(self.description) + ">"

    def shortDescription(self):
        return None

    def countTestCases(self):
        return 0

    def run(self, result):
        pass

    __call__ = run


def _boxedFixturePhase(hook, method, parent, result):
    if hook is None:
        return True
    try:
        hook()
    except Exception as exc:
        if isinstance(result, _DebugResult):
            raise
        _boxedFixtureProblem(parent, method, result, exc, exc.__traceback__)
        return False
    return True


def _boxedFixtureProblem(parent, method, result, exc, tb):
    label = method + " (" + parent + ")"
    holder = _ErrorHolder(label)
    if isinstance(exc, SkipTest):
        result.addSkip(holder, str(exc))
    else:
        result.addError(holder, (type(exc), exc, tb))


def _boxedClassPhase(cls, method, result):
    return _boxedFixturePhase(getattr(cls, method, None), method, cls.__module__ + "." + cls.__qualname__, result)


def _boxedClassCleanups(cls, method, result):
    hook = getattr(cls, "doClassCleanups", None)
    if hook is not None:
        hook()
        for _, exc, tb in cls.tearDown_exceptions:
            if isinstance(result, _DebugResult):
                raise exc
            _boxedFixtureProblem(cls.__module__ + "." + cls.__qualname__, method, result, exc, tb)


def _boxedTearDownClass(cls, result):
    if (cls is not None and not getattr(result, "_moduleSetUpFailed", False)
            and not getattr(cls, "_classSetupFailed", False)
            and not getattr(cls, "__unittest_skip__", False)):
        _boxedClassPhase(cls, "tearDownClass", result)
        _boxedClassCleanups(cls, "tearDownClass", result)


def _boxedTearDownModule(previous, result):
    if previous is None or getattr(result, "_moduleSetUpFailed", False):
        return
    name = previous.__module__
    try:
        module = sys.modules[name]
    except KeyError:
        return
    _boxedFixturePhase(getattr(module, "tearDownModule", None), "tearDownModule", name, result)
    _boxedFixturePhase(case.doModuleCleanups, "tearDownModule", name, result)


def _boxedSetUpModule(previous, cls, result):
    name = cls.__module__
    if previous is not None and name == previous.__module__:
        return
    _boxedTearDownModule(previous, result)
    result._moduleSetUpFailed = False
    try:
        module = sys.modules[name]
    except KeyError:
        return
    if not _boxedFixturePhase(getattr(module, "setUpModule", None), "setUpModule", name, result):
        result._moduleSetUpFailed = True
        _boxedFixturePhase(case.doModuleCleanups, "setUpModule", name, result)


def _boxedIsLeaf(test):
    try:
        iter(test)
    except TypeError:
        return True
    return False


class _DebugResult:
    _previousTestClass = None
    _moduleSetUpFailed = False
    shouldStop = False


def _suiteDebug(self):
    self.run(_DebugResult(), True)


def _suiteRun(self, result, debug=False):
    outer = not getattr(result, "_testRunEntered", False)
    if outer:
        result._testRunEntered = True
    try:
        for index, test in enumerate(self):
            if result.shouldStop:
                break
            if _boxedIsLeaf(test):
                cls = type(test)
                if not debug:
                    _boxedCheckFixtures(cls)
                previous = getattr(result, "_previousTestClass", None)
                if cls is not previous:
                    _boxedTearDownClass(previous, result)
                    _boxedSetUpModule(previous, cls, result)
                    if not getattr(result, "_moduleSetUpFailed", False) and not getattr(cls, "__unittest_skip__", False):
                        try:
                            cls._classSetupFailed = False
                        except TypeError:
                            pass  # Builtin callables can be suite members.
                        if not _boxedClassPhase(cls, "setUpClass", result):
                            cls._classSetupFailed = True
                            _boxedClassCleanups(cls, "setUpClass", result)
                    result._previousTestClass = cls
                if getattr(cls, "_classSetupFailed", False) or getattr(result, "_moduleSetUpFailed", False):
                    continue
            if debug:
                test.debug()
            else:
                test(result)
            if self._cleanup:
                self._removeTestAtIndex(index)
        if outer:
            previous = getattr(result, "_previousTestClass", None)
            _boxedTearDownClass(previous, result)
            _boxedTearDownModule(previous, result)
    finally:
        if outer:
            result._testRunEntered = False
    return result


TestCase.__init_subclass__ = classmethod(_initCaseSubclass)
TestCase.addClassCleanup = classmethod(_addClassCleanup)
TestCase.doClassCleanups = classmethod(_doClassCleanups)
TestCase.enterClassContext = classmethod(_enterClassContext)

TestCase.__eq__ = _caseEqual
TestCase.__hash__ = _caseHash
TestCase.shortDescription = _caseShortDescription
TestCase.__str__ = _caseStr
TestCase._callSetUp = _caseCallSetUp
TestCase._callTestMethod = _caseCallTestMethod
TestCase._callTearDown = _caseCallTearDown
TestCase.debug = _caseDebug
TestCase.run = _caseRun
TestCase.__call__ = _callTest
TestSuite._cleanup = True
TestSuite.__init__ = _suiteInit
TestSuite.__iter__ = _suiteIter
TestSuite.__eq__ = _suiteEqual
TestSuite.__hash__ = None
TestSuite.countTestCases = _suiteCount
TestSuite.addTest = _suiteAddTest
TestSuite.addTests = _suiteAddTests
TestSuite._removeTestAtIndex = _suiteRemoveTest
TestSuite.debug = _suiteDebug
TestSuite.run = _suiteRun
TestSuite.__call__ = _callTest
