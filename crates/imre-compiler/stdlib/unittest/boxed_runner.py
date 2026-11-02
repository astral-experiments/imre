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
    self.buffer = False
    self.tb_locals = False


def _printErrors(self):
    pass


def _textPrintErrors(self):
    if self.verbose:
        self.stream.writeln()
    self._reportProblems("ERROR", self._errorNames, self.errors)
    self._reportProblems("FAIL", self._failureNames, self.failures)


def _wrappedStream(self):
    return sys.stderr if self._memory is None else self._memory


def _streamAttribute(self, name):
    if name in ("stream", "__getstate__"):
        raise AttributeError(name)
    return getattr(self.stream, name)


def _boxedRunnerInit(self, stream=None, descriptions=True, verbosity=1, failfast=False,
                     buffer=False, resultclass=None, warnings=None, *, tb_locals=False,
                     durations=None):
    if buffer or tb_locals or durations is not None:
        raise NotImplementedError("native TextTestRunner does not yet support output buffering, "
                                  "local variables in tracebacks or durations")
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
TestResult.__init__ = _boxedResultInit
TestResult.addError = _addError
TestResult.addFailure = _addFailure
TestResult._recordException = _recordBoxedException
TestResult.printErrors = _printErrors
TextTestResult.__init__ = _boxedTextResultInit
TextTestResult.addError = _textError
TextTestResult.addFailure = _textFailure
TextTestResult.printErrors = _textPrintErrors
TextTestResult.separator2 = "----------------------------------------------------------------------"
TextTestRunner.resultclass = TextTestResult
TextTestRunner.__init__ = _boxedRunnerInit
TextTestRunner._makeResult = _makeResult
TextTestRunner.run = _runWithResult
