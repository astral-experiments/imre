# Fixed-layout TestResult specializes its error representation. Boxed results
# receive the real exception tuple through the public overridable callbacks.
# Traceback filtering from CPython 3.14.5 Lib/unittest/result.py. See ../docstrings/LICENSE.
def _clean_tracebacks(self, exctype, value, tb, test):
    ret = None
    first = True
    excs = [(exctype, value, tb)]
    seen = {id(value)}  # Detect loops in chained exceptions.
    while excs:
        (exctype, value, tb) = excs.pop()
        # Skip test runner traceback levels
        while tb and self._is_relevant_tb_level(tb):
            tb = tb.tb_next

        # Skip assert*() traceback levels
        if exctype is test.failureException:
            self._remove_unittest_tb_frames(tb)

        if first:
            ret = tb
            first = False
        else:
            value.__traceback__ = tb

        if value is not None:
            for c in (value.__cause__, value.__context__):
                if c is not None and id(c) not in seen:
                    excs.append((type(c), c, c.__traceback__))
                    seen.add(id(c))
    return ret


def _is_relevant_tb_level(self, tb):
    return '__unittest' in tb.tb_frame.f_globals


def _remove_unittest_tb_frames(self, tb):
    '''Truncates usercode tb at the first unittest frame.

    If the first frame of the traceback is in user code,
    the prefix up to the first unittest frame is returned.
    If the first frame is already in the unittest module,
    the traceback is not modified.
    '''
    prev = None
    while tb and not self._is_relevant_tb_level(tb):
        prev = tb
        tb = tb.tb_next
    if prev is not None:
        prev.tb_next = None


def _exc_info_to_string(self, err, test):
    import traceback
    kind, value, tb = err
    tb = self._clean_tracebacks(kind, value, tb, test)
    message = "".join(traceback.format_exception(kind, value, tb))
    if self.buffer:
        output = sys.stdout.getvalue()
        error = sys.stderr.getvalue()
        if output:
            message += _bufferedOutput(output, "Stdout")
        if error:
            message += _bufferedOutput(error, "Stderr")
    return message


def _addError(self, test, error):
    if getattr(self, "failfast", False):
        self.stop()
    self.errors.append((test, self._exc_info_to_string(error, test)))
    self._mirrorOutput = True


def _addFailure(self, test, error):
    if getattr(self, "failfast", False):
        self.stop()
    self.failures.append((test, self._exc_info_to_string(error, test)))
    self._mirrorOutput = True


def _resultExpectedFailure(self, test, error):
    self.expectedFailures.append((test, self._exc_info_to_string(error, test)))


def _resultSubTest(self, test, subtest, error):
    if error is not None:
        if getattr(self, "failfast", False):
            self.stop()
        problems = self.failures if issubclass(error[0], test.failureException) else self.errors
        problems.append((subtest, self._exc_info_to_string(error, test)))
        self._mirrorOutput = True


# Adapted from CPython 3.14.5 Lib/unittest/runner.py. See ../docstrings/LICENSE.
def getDescription(self, test):
    description = test.shortDescription()
    if self.descriptions and description:
        return "\n".join((str(test), description))
    return str(test)


def _textStartTest(self, test):
    TestResult.startTest(self, test)
    if self.showAll:
        self.stream.write(self.getDescription(test))
        self.stream.write(" ... ")
        self.stream.flush()
        self._newline = False


def _write_status(self, test, status):
    subtest = isinstance(test, case._SubTest)
    if subtest or self._newline:
        if not self._newline:
            self.stream.writeln()
        if subtest:
            self.stream.write("  ")
        self.stream.write(self.getDescription(test))
        self.stream.write(" ... ")
    self.stream.writeln(status)
    self.stream.flush()
    self._newline = True


def _textStatus(self, test, short, status):
    if self.showAll:
        self._write_status(test, status)
    elif self.dots:
        self.stream.write(short)
        self.stream.flush()


def _textSuccess(self, test):
    TestResult.addSuccess(self, test)
    _textStatus(self, test, ".", "ok")


def _textSkip(self, test, reason):
    TestResult.addSkip(self, test, reason)
    status = "skipped " + repr(reason) if self.showAll else ""
    _textStatus(self, test, "s", status)


def _textSubTest(self, test, subtest, error):
    if error is not None:
        if issubclass(error[0], subtest.failureException):
            _textStatus(self, subtest, "F", "FAIL")
        else:
            _textStatus(self, subtest, "E", "ERROR")
    _resultSubTest(self, test, subtest, error)


def _textExpectedStatus(self, short, status):
    if self.showAll:
        self.stream.writeln(status)
    elif self.dots:
        self.stream.write(short)
    else:
        return
    self.stream.flush()


def _textExpectedFailure(self, test, error):
    _resultExpectedFailure(self, test, error)
    _textExpectedStatus(self, "x", "expected failure")


def _textUnexpectedSuccess(self, test):
    TestResult.addUnexpectedSuccess(self, test)
    _textExpectedStatus(self, "u", "unexpected success")


def _recordBoxedException(self, case, exc, detail):
    if isinstance(exc, case.failureException):
        self.addFailure(case, sys.exc_info())
    else:
        self.addError(case, sys.exc_info())


def _textError(self, test, error):
    _addError(self, test, error)
    _textStatus(self, test, "E", "ERROR")


def _textFailure(self, test, error):
    _addFailure(self, test, error)
    _textStatus(self, test, "F", "FAIL")


_resultInit = TestResult.__init__


def _boxedResultInit(self, stream=None, descriptions=None, verbosity=None):
    _resultInit(self)
    self.expectedFailures = []
    self.collectedDurations = []
    self.buffer = False
    self.tb_locals = False
    self._stdout_buffer = None
    self._stderr_buffer = None
    self._original_stdout = sys.stdout
    self._original_stderr = sys.stderr
    self._mirrorOutput = False


# Adapted from CPython 3.14.5 Lib/unittest/result.py. See ../docstrings/LICENSE.
def _bufferedOutput(output, label):
    if not output.endswith("\n"):
        output += "\n"
    return "\n%s:\n%s" % (label, output)


def _setupStdout(self):
    if self.buffer:
        if self._stderr_buffer is None:
            import io
            self._stderr_buffer = io.StringIO()
            self._stdout_buffer = io.StringIO()
        sys.stdout = self._stdout_buffer
        sys.stderr = self._stderr_buffer


def _restoreStdout(self):
    if self.buffer:
        if self._mirrorOutput:
            output = sys.stdout.getvalue()
            error = sys.stderr.getvalue()
            if output:
                self._original_stdout.write(_bufferedOutput(output, "Stdout"))
            if error:
                self._original_stderr.write(_bufferedOutput(error, "Stderr"))
        sys.stdout = self._original_stdout
        sys.stderr = self._original_stderr
        self._stdout_buffer.seek(0)
        self._stdout_buffer.truncate()
        self._stderr_buffer.seek(0)
        self._stderr_buffer.truncate()


def startTest(self, test):
    self.testsRun += 1
    self._mirrorOutput = False
    self._setupStdout()


def stopTest(self, test):
    self._restoreStdout()
    self._mirrorOutput = False


def _wasSuccessful(self):
    """Tells whether or not this result was a success."""
    return (len(self.failures) == len(self.errors) == 0
            and (not hasattr(self, "unexpectedSuccesses") or len(self.unexpectedSuccesses) == 0))


def _printErrors(self):
    pass


def printErrorList(self, flavour, errors):
    for test, detail in errors:
        self.stream.writeln(self.separator1)
        self.stream.writeln(f"{flavour}: {self.getDescription(test)}")
        self.stream.writeln(self.separator2)
        self.stream.writeln("%s" % detail)
        self.stream.flush()


def _textPrintErrors(self):
    if self.showAll or self.dots:
        self.stream.writeln()
        self.stream.flush()
    self.printErrorList("ERROR", self.errors)
    self.printErrorList("FAIL", self.failures)
    unexpected = getattr(self, "unexpectedSuccesses", ())
    if unexpected:
        self.stream.writeln(self.separator1)
        for test in unexpected:
            self.stream.writeln(f"UNEXPECTED SUCCESS: {self.getDescription(test)}")
        self.stream.flush()


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
    if tb_locals:
        raise NotImplementedError("native TextTestRunner does not yet support local variables in tracebacks")
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
    self.showAll = verbosity > 1
    self.dots = verbosity == 1
    self._newline = True
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
TestResult._setupStdout = _setupStdout
TestResult._restoreStdout = _restoreStdout
TestResult.startTest = startTest
TestResult.stopTest = stopTest
TestResult._clean_tracebacks = _clean_tracebacks
TestResult._is_relevant_tb_level = _is_relevant_tb_level
TestResult._remove_unittest_tb_frames = _remove_unittest_tb_frames
TestResult._exc_info_to_string = _exc_info_to_string
TestResult.addExpectedFailure = _resultExpectedFailure
TestResult.addSubTest = _resultSubTest
TestResult.addError = _addError
TestResult.addFailure = _addFailure
TestResult._recordException = _recordBoxedException
_wasSuccessful.__name__ = "wasSuccessful"
_wasSuccessful.__qualname__ = "TestResult.wasSuccessful"
TestResult.wasSuccessful = _wasSuccessful
TestResult.printErrors = _printErrors
TextTestResult.__init__ = _boxedTextResultInit
getDescription.__qualname__ = "TextTestResult.getDescription"
TextTestResult.getDescription = getDescription
TextTestResult._write_status = _write_status
TextTestResult.startTest = _textStartTest
TextTestResult.addSuccess = _textSuccess
TextTestResult.addSkip = _textSkip
TextTestResult.addSubTest = _textSubTest
TextTestResult.addExpectedFailure = _textExpectedFailure
TextTestResult.addUnexpectedSuccess = _textUnexpectedSuccess
TextTestResult.addError = _textError
TextTestResult.addFailure = _textFailure
printErrorList.__qualname__ = "TextTestResult.printErrorList"
TextTestResult.printErrorList = printErrorList
TextTestResult.printErrors = _textPrintErrors
TextTestResult.separator1 = "=" * 70
TextTestResult.separator2 = "----------------------------------------------------------------------"
TextTestRunner.resultclass = TextTestResult
TextTestRunner.__init__ = _boxedRunnerInit
TextTestRunner._makeResult = _makeResult
TextTestRunner._printDurations = _printDurations
TextTestRunner.run = _runWithResult
