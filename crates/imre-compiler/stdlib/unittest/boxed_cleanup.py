# A cleanup may be any callable, with its original positional and keyword arguments.
def addCleanup(self, function, /, *args, **kwargs):
    if not hasattr(self, "_cleanups"):
        self._cleanups = []
    self._cleanups.append((function, args, kwargs))


def enterContext(self, cm):
    cls = type(cm)
    try:
        enter = cls.__enter__
        exit = cls.__exit__
    except AttributeError:
        message = (f"'{cls.__module__}.{cls.__qualname__}' object does "
                   "not support the context manager protocol")
        if hasattr(cls, "__aenter__") and hasattr(cls, "__aexit__"):
            message += (" but it supports the asynchronous context manager protocol. "
                        "Did you mean to use enterAsyncContext()?")
        raise TypeError(message) from None
    result = enter(cm)
    self.addCleanup(exit, cm, None, None, None)
    return result


def _callCleanup(self, function, /, *args, **kwargs):
    function(*args, **kwargs)


def doCleanups(self):
    cleanups = getattr(self, "_cleanups", ())
    outcome = getattr(self, "_outcome", None)
    success = True
    while cleanups:
        function, args, kwargs = cleanups.pop()
        if outcome is not None:
            with outcome.testPartExecutor(self):
                self._callCleanup(function, *args, **kwargs)
        elif self._runner is not None:
            def call_cleanup():
                self._callCleanup(function, *args, **kwargs)
            if not self._runner._phase(call_cleanup, self):
                success = False
        else:
            try:
                self._callCleanup(function, *args, **kwargs)
            except KeyboardInterrupt:
                raise
            except BaseException:
                success = False
    return outcome.success if outcome is not None else success


def _runCleanups(self, case):
    return case.doCleanups()


def setUpClass(cls):
    """Hook method for setting up class fixture before running tests in the class."""


def tearDownClass(cls):
    """Hook method for deconstructing the class fixture after running all tests in the class."""


TestCase.setUpClass = classmethod(setUpClass)
TestCase.tearDownClass = classmethod(tearDownClass)
TestCase.addCleanup = addCleanup
TestCase.enterContext = enterContext
TestCase._callCleanup = _callCleanup
TestCase.doCleanups = doCleanups

TestResult._runCleanups = _runCleanups
