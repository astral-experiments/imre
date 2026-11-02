# Fixed-layout TestResult specializes its error representation. Boxed results
# receive the real exception tuple through the public overridable callbacks.
def _boxedResultDetail(error):
    import traceback
    kind, value, tb = error
    # Only trim leading frames in the bundled unittest implementation. A user
    # calling addError directly can supply a traceback with no framework frame.
    internal = (_boxedResultDetail.__code__.co_filename, case.__file__)
    while tb is not None and tb.tb_frame.f_code.co_filename in internal:
        tb = tb.tb_next
    return "".join(traceback.format_exception(kind, value, tb))


class _CaseResultDispatch(TestResult):
    # Case phases use private helpers; public callbacks and state belong to
    # the observer supplied by the runner, even for a duck-typed result.
    def __init__(self, observer):
        self._observer = observer
        for name in ("startTest", "stopTest", "addSuccess", "addError", "addFailure", "stop"):
            setattr(self, name, getattr(observer, name))
        if hasattr(observer, "addSkip"):
            self.addSkip = observer.addSkip
        self.skipped = getattr(observer, "skipped", ())

    def __getattr__(self, name):
        return getattr(self._observer, name)


def _addError(self, test, error):
    if self.failfast:
        self.stop()
    self.errors.append((test, _boxedResultDetail(error)))


def _addFailure(self, test, error):
    if self.failfast:
        self.stop()
    self.failures.append((test, _boxedResultDetail(error)))


def _resultExpectedFailure(self, test, error):
    self.expectedFailures.append((test, _boxedResultDetail(error)))


def _resultSubTest(self, test, subtest, error):
    if error is not None:
        if self.failfast:
            self.stop()
        problems = self.failures if issubclass(error[0], test.failureException) else self.errors
        problems.append((subtest, _boxedResultDetail(error)))


def _textSubTest(self, test, subtest, error):
    if error is not None:
        if issubclass(error[0], subtest.failureException):
            self._markSubTest(subtest._subDescription(), "F", "FAIL")
        else:
            self._markSubTest(subtest._subDescription(), "E", "ERROR")
    _resultSubTest(self, test, subtest, error)


def _textExpectedFailure(self, test, error):
    _resultExpectedFailure(self, test, error)
    self._mark("x", "expected failure")


def _textUnexpectedSuccess(self, test):
    TestResult.addUnexpectedSuccess(self, test)
    self._mark("u", "unexpected success")


def _recordBoxedException(self, case, exc, detail):
    if isinstance(exc, case.failureException):
        self.addFailure(case, sys.exc_info())
    else:
        self.addError(case, sys.exc_info())


def _textError(self, test, error):
    _addError(self, test, error)
    self._mark("E", "ERROR")


def _textFailure(self, test, error):
    _addFailure(self, test, error)
    self._mark("F", "FAIL")


_resultInit = TestResult.__init__


def _boxedResultInit(self, stream=None, descriptions=None, verbosity=None):
    _resultInit(self)
    self.expectedFailures = []
    self.collectedDurations = []
    self.buffer = False
    self.tb_locals = False


def _printErrors(self):
    pass


def _textPrintErrors(self):
    if self.verbose:
        self.stream.writeln()
    self._reportProblems("ERROR", self._errorNames, self.errors)
    self._reportProblems("FAIL", self._failureNames, self.failures)
    if self.unexpectedSuccesses:
        self.stream.writeln("=" * 70)
        for test in self.unexpectedSuccesses:
            name = str(test)
            if self.descriptions:
                description = test.shortDescription()
                if description:
                    name += "\n" + description
            self.stream.writeln("UNEXPECTED SUCCESS: " + name)


def _wrappedStream(self):
    return sys.stderr if self._memory is None else self._memory


class _StreamWrite:
    # Keep instance overrides while delegating to the stream's current bound method.
    def __get__(self, instance, owner):
        if instance is None:
            raise AttributeError(f"type object '{owner.__name__}' has no attribute 'write'")
        return instance.stream.write


def _streamAttribute(self, name):
    if name in ("stream", "__getstate__"):
        raise AttributeError(name)
    return getattr(self.stream, name)


def _boxedRunnerInit(self, stream=None, descriptions=True, verbosity=1, failfast=False,
                     buffer=False, resultclass=None, warnings=None, *, tb_locals=False,
                     durations=None):
    if buffer or tb_locals:
        raise NotImplementedError("native TextTestRunner does not yet support output buffering "
                                  "or local variables in tracebacks")
    self.stream = _WritelnDecorator(sys.stderr if stream is None else stream)
    self.descriptions = descriptions
    self.verbosity = verbosity
    self.failfast = failfast
    self.buffer = buffer
    self.tb_locals = tb_locals
    self.durations = durations
    self.warnings = warnings
    if resultclass is not None:
        self.resultclass = resultclass


def _makeResult(self):
    try:
        return self.resultclass(self.stream, self.descriptions, self.verbosity, durations=self.durations)
    except TypeError:
        return self.resultclass(self.stream, self.descriptions, self.verbosity)


_textResultInit = TextTestResult.__init__


def _boxedTextResultInit(self, stream=None, descriptions=True, verbosity=1, *, durations=None):
    _textResultInit(self, stream, descriptions, verbosity)
    if isinstance(stream, _WritelnDecorator):
        self.stream = stream
    self.durations = durations


from unittest import runner as _runnerModule


# From CPython 3.14.5 Lib/unittest/{result,runner}.py. See ../docstrings/LICENSE.
def addDuration(self, test, elapsed):
    """Called when a test finished to run, regardless of its outcome.
    *test* is the test case corresponding to the test method.
    *elapsed* is the time represented in seconds, and it includes the
    execution of cleanup functions.
    """
    # support for a TextTestRunner using an old TestResult class
    if hasattr(self, "collectedDurations"):
        # Pass test repr and not the test object itself to avoid resources leak
        self.collectedDurations.append((str(test), elapsed))


def _printDurations(self, result):
    if not result.collectedDurations:
        return
    ls = sorted(result.collectedDurations, key=lambda x: x[1],
                reverse=True)
    if self.durations > 0:
        ls = ls[:self.durations]
    self.stream.writeln("Slowest test durations")
    if hasattr(result, 'separator2'):
        self.stream.writeln(result.separator2)
    hidden = False
    for test, elapsed in ls:
        if self.verbosity < 2 and elapsed < 0.001:
            hidden = True
            continue
        self.stream.writeln("%-10s %s" % ("%.3fs" % elapsed, test))
    if hidden:
        self.stream.writeln("\n(durations < 0.001s were hidden; "
                            "use -v to show these durations)")
    else:
        self.stream.writeln("")


def _runWithResult(self, test):
    result = self._makeResult()
    _runnerModule.registerResult(result)
    result.failfast = self.failfast
    result.buffer = self.buffer
    result.tb_locals = self.tb_locals
    with _runnerModule.warnings.catch_warnings():
        if self.warnings:
            _runnerModule.warnings.simplefilter(self.warnings)
        started = time.time()
        start = getattr(result, "startTestRun", None)
        if start is not None:
            start()
        try:
            test(result)
        finally:
            stop = getattr(result, "stopTestRun", None)
            if stop is not None:
                stop()
        elapsed = time.time() - started
    result.printErrors()
    if self.durations is not None:
        self._printDurations(result)
    if hasattr(result, "separator2"):
        self.stream.writeln(result.separator2)
    run = result.testsRun
    plural = "" if run == 1 else "s"
    self.stream.writeln(f"Ran {run} test{plural} in {elapsed:.3f}s")
    self.stream.writeln()
    infos = []
    skipped = len(getattr(result, "skipped", ()))
    if not result.wasSuccessful():
        status = "FAILED"
        if result.failures:
            infos.append("failures=" + str(len(result.failures)))
        if result.errors:
            infos.append("errors=" + str(len(result.errors)))
    elif not run and not skipped:
        status = "NO TESTS RAN"
    else:
        status = "OK"
    if skipped:
        infos.append("skipped=" + str(skipped))
    for attr, label in (("expectedFailures", "expected failures"),
                        ("unexpectedSuccesses", "unexpected successes")):
        count = len(getattr(result, attr, ()))
        if count:
            infos.append(label + "=" + str(count))
    if infos:
        status += " (" + ", ".join(infos) + ")"
    self.stream.writeln(status)
    self.stream.flush()
    return result


_WritelnDecorator.stream = property(_wrappedStream)
_WritelnDecorator.__getattr__ = _streamAttribute
_WritelnDecorator.write = _StreamWrite()
TestResult.addDuration = addDuration
TestResult.__init__ = _boxedResultInit
TestResult.addExpectedFailure = _resultExpectedFailure
TestResult.addSubTest = _resultSubTest
TestResult.addError = _addError
TestResult.addFailure = _addFailure
TestResult._recordException = _recordBoxedException
TestResult.printErrors = _printErrors
TextTestResult.__init__ = _boxedTextResultInit
TextTestResult.addSubTest = _textSubTest
TextTestResult.addExpectedFailure = _textExpectedFailure
TextTestResult.addUnexpectedSuccess = _textUnexpectedSuccess
TextTestResult.addError = _textError
TextTestResult.addFailure = _textFailure
TextTestResult.printErrors = _textPrintErrors
TextTestResult.separator2 = "----------------------------------------------------------------------"
TextTestRunner.resultclass = TextTestResult
TextTestRunner.__init__ = _boxedRunnerInit
TextTestRunner._makeResult = _makeResult
TextTestRunner._printDurations = _printDurations
TextTestRunner.run = _runWithResult
