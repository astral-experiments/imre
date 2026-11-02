# Exception and warning classes and callable arguments use the boxed Python object protocol.
def _boxed_format_message(self, msg, standardMsg):
    # In short-message mode the original message object becomes the exception argument.
    if not self.longMessage:
        return msg or standardMsg
    if msg is None:
        return standardMsg
    return standardMsg + " : " + str(msg)


def _is_assertion_subtype(expected, base):
    if isinstance(expected, tuple):
        return all(_is_assertion_subtype(item, base) for item in expected)
    return isinstance(expected, type) and issubclass(expected, base)


class _BoxedAssertionContext:
    def __init__(self, case, expected):
        self.test_case = case
        self.expected = expected
        self.msg = None
        self.callable_name = None

    def handle(self, args, kwargs):
        if not args:
            self.msg = kwargs.pop("msg", None)
            if kwargs:
                raise TypeError(repr(next(iter(kwargs))) + " is an invalid keyword argument for this function")
            return self
        callback = args[0]
        try:
            self.callable_name = callback.__name__
        except AttributeError:
            self.callable_name = str(callback)
        with self:
            callback(*args[1:], **kwargs)


class _BoxedRaisesContext(_BoxedAssertionContext):
    def __init__(self, case, expected, regex):
        super().__init__(case, expected)
        self.expected_regex = regex

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        if exc_type is None:
            name = getattr(self.expected, "__name__", None)
            if name is None:
                name = str(self.expected)
            message = name + " not raised"
            if self.callable_name:
                message += " by {}".format(self.callable_name)
            raise self.test_case.failureException(self.test_case._formatMessage(self.msg, message))
        import traceback
        traceback.clear_frames(tb)
        if not issubclass(exc_type, self.expected):
            return False
        exc.__traceback__ = None
        self.exception = exc
        if self.expected_regex is not None and not self.expected_regex.search(str(exc)):
            message = '"{}" does not match "{}"'.format(self.expected_regex.pattern, str(exc))
            raise self.test_case.failureException(self.test_case._formatMessage(self.msg, message))
        return True


class _BoxedWarnsContext(_BoxedAssertionContext):
    def __enter__(self):
        import warnings

        # Reusing a test in the same process must not hide a previously emitted warning.
        for module in list(sys.modules.values()):
            if getattr(module, "__warningregistry__", None):
                module.__warningregistry__ = {}
        self.warnings_manager = warnings.catch_warnings(record=True)
        self.warnings = self.warnings_manager.__enter__()
        warnings.simplefilter("always", self.expected)
        return self

    def __exit__(self, exc_type, exc, tb):
        self.warnings_manager.__exit__(exc_type, exc, tb)
        if exc_type is not None:
            return
        for record in self.warnings:
            if isinstance(record.message, self.expected):
                self.warning = record.message
                self.filename = record.filename
                self.lineno = record.lineno
                return
        name = getattr(self.expected, "__name__", None)
        message = (str(self.expected) if name is None else name) + " not triggered"
        if self.callable_name:
            message += " by {}".format(self.callable_name)
        raise self.test_case.failureException(self.test_case._formatMessage(self.msg, message))


def _boxedRaises(self, method, expected_exception, regex, args, kwargs):
    if not _is_assertion_subtype(expected_exception, BaseException):
        raise TypeError(method + "() arg 1 must be an exception type or tuple of exception types")
    return _BoxedRaisesContext(self, expected_exception, regex).handle(args, kwargs)


def assertWarns(self, expected_warning, *args, **kwargs):
    if not _is_assertion_subtype(expected_warning, Warning):
        raise TypeError("assertWarns() arg 1 must be a warning type or tuple of warning types")
    return _BoxedWarnsContext(self, expected_warning).handle(args, kwargs)


def assertRaises(self, expected_exception, *args, **kwargs):
    return _boxedRaises(self, "assertRaises", expected_exception, None, args, kwargs)


def assertRaisesRegex(self, expected_exception, expected_regex, *args, **kwargs):
    regex = re.compile(expected_regex) if expected_regex is not None else None
    return _boxedRaises(self, "assertRaisesRegex", expected_exception, regex, args, kwargs)


def assertRegex(self, text, expected_regex, msg=None):
    """Fail the test unless the text matches the regular expression."""
    if isinstance(expected_regex, (str, bytes)):
        assert expected_regex, "expected_regex must not be empty."
        expected_regex = re.compile(expected_regex)
    if not expected_regex.search(text):
        standardMsg = "Regex didn't match: %r not found in %r" % (
            expected_regex.pattern, text)
        # _formatMessage ensures the longMessage option is respected
        msg = self._formatMessage(msg, standardMsg)
        raise self.failureException(msg)

TestCase.assertRegex = assertRegex
TestCase.assertRaisesRegex = assertRaisesRegex
TestCase._formatMessage = _boxed_format_message
TestCase.assertRaises = assertRaises
TestCase.assertWarns = assertWarns
del assertRegex, assertRaises, assertRaisesRegex, assertWarns
