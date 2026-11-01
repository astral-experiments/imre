# Exception classes and callable arguments use the boxed Python object protocol.
def _boxed_format_message(self, msg, standardMsg):
    # In short-message mode the original message object becomes the exception argument.
    if not self.longMessage:
        return msg or standardMsg
    if msg is None:
        return standardMsg
    return standardMsg + " : " + str(msg)


def _is_exception_type(expected):
    if isinstance(expected, tuple):
        return all(_is_exception_type(item) for item in expected)
    return isinstance(expected, type) and issubclass(expected, BaseException)


class _BoxedRaisesContext:
    def __init__(self, case, expected, msg, regex, callable_name=None):
        self.test_case = case
        self.expected = expected
        self.msg = msg
        self.callable_name = callable_name
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
                message += " by " + self.callable_name
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


def _boxedRaises(self, method, expected_exception, regex, args, kwargs):
    if not _is_exception_type(expected_exception):
        raise TypeError(method + "() arg 1 must be an exception type or tuple of exception types")
    if not args:
        msg = kwargs.pop("msg", None)
        if kwargs:
            raise TypeError(repr(next(iter(kwargs))) + " is an invalid keyword argument for this function")
        return _BoxedRaisesContext(self, expected_exception, msg, regex)
    callback = args[0]
    name = getattr(callback, "__name__", None)
    if name is None:
        name = str(callback)
    with _BoxedRaisesContext(self, expected_exception, None, regex, name):
        callback(*args[1:], **kwargs)


def assertRaises(self, expected_exception, *args, **kwargs):
    return _boxedRaises(self, "assertRaises", expected_exception, None, args, kwargs)


def assertRaisesRegex(self, expected_exception, expected_regex, *args, **kwargs):
    regex = re.compile(expected_regex) if expected_regex is not None else None
    return _boxedRaises(self, "assertRaisesRegex", expected_exception, regex, args, kwargs)


TestCase.assertRaisesRegex = assertRaisesRegex
TestCase._formatMessage = _boxed_format_message
TestCase.assertRaises = assertRaises
del assertRaises, assertRaisesRegex
