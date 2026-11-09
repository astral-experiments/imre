# Exception classes and callable arguments use the boxed Python object protocol.
import traceback as _traceback


def _boxed_format_message(self, msg, standardMsg):
    # In short-message mode the original message object becomes the exception argument.
    if not self.longMessage:
        return msg or standardMsg
    if msg is None:
        return standardMsg
    return standardMsg + " : " + str(msg)


def _is_subtype(expected, basetype):
    if isinstance(expected, tuple):
        return all(_is_subtype(e, basetype) for e in expected)
    return isinstance(expected, type) and issubclass(expected, basetype)


class _BoxedAssertionContext:
    def __init__(self, case, expected, msg, regex, callable_name=None):
        self.test_case = case
        self.expected = expected
        self.msg = msg
        self.callable_name = callable_name
        self.expected_regex = regex

    def _raiseFailure(self, message):
        raise self.test_case.failureException(self.test_case._formatMessage(self.msg, message))


class _BoxedRaisesContext(_BoxedAssertionContext):
    _base_type = BaseException
    _base_type_str = "an exception type or tuple of exception types"

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
            self._raiseFailure(message)
        _traceback.clear_frames(tb)
        if not issubclass(exc_type, self.expected):
            return False
        exc.__traceback__ = None
        self.exception = exc
        if self.expected_regex is not None and not self.expected_regex.search(str(exc)):
            message = '"{}" does not match "{}"'.format(self.expected_regex.pattern, str(exc))
            self._raiseFailure(message)
        return True


# From CPython 3.14.5 Lib/unittest/case.py. See ../docstrings/LICENSE.
class _AssertWarnsContext(_BoxedAssertionContext):
    """A context manager used to implement TestCase.assertWarns* methods."""

    _base_type = Warning
    _base_type_str = 'a warning type or tuple of warning types'

    def __enter__(self):
        import sys
        import warnings

        # The __warningregistry__'s need to be in a pristine state for tests
        # to work properly.
        for v in list(sys.modules.values()):
            if getattr(v, '__warningregistry__', None):
                v.__warningregistry__ = {}
        self.warnings_manager = warnings.catch_warnings(record=True)
        self.warnings = self.warnings_manager.__enter__()
        warnings.simplefilter("always", self.expected)
        return self

    def __exit__(self, exc_type, exc_value, tb):
        self.warnings_manager.__exit__(exc_type, exc_value, tb)
        if exc_type is not None:
            # let unexpected exceptions pass through
            return
        try:
            exc_name = self.expected.__name__
        except AttributeError:
            exc_name = str(self.expected)
        first_matching = None
        for m in self.warnings:
            w = m.message
            if not isinstance(w, self.expected):
                continue
            if first_matching is None:
                first_matching = w
            if (self.expected_regex is not None and
                not self.expected_regex.search(str(w))):
                continue
            # store warning for later retrieval
            self.warning = w
            self.filename = m.filename
            self.lineno = m.lineno
            return
        # Now we simply try to choose a helpful failure message
        if first_matching is not None:
            self._raiseFailure('"{}" does not match "{}"'.format(
                     self.expected_regex.pattern, str(first_matching)))
        if self.callable_name:
            self._raiseFailure("{} not triggered by {}".format(exc_name,
                                                               self.callable_name))
        else:
            self._raiseFailure("{} not triggered".format(exc_name))


class _AssertNotWarnsContext(_AssertWarnsContext):

    def __exit__(self, exc_type, exc_value, tb):
        self.warnings_manager.__exit__(exc_type, exc_value, tb)
        if exc_type is not None:
            # let unexpected exceptions pass through
            return
        try:
            exc_name = self.expected.__name__
        except AttributeError:
            exc_name = str(self.expected)
        for m in self.warnings:
            w = m.message
            if isinstance(w, self.expected):
                self._raiseFailure(f"{exc_name} triggered")


def _boxedAssertion(self, context, method, expected, regex, args, kwargs):
    if not _is_subtype(expected, context._base_type):
        raise TypeError(method + "() arg 1 must be " + context._base_type_str)
    if not args:
        msg = kwargs.pop("msg", None)
        if kwargs:
            raise TypeError(repr(next(iter(kwargs))) + " is an invalid keyword argument for this function")
        return context(self, expected, msg, regex)
    callback = args[0]
    try:
        name = callback.__name__
    except AttributeError:
        name = str(callback)
    with context(self, expected, None, regex, name):
        callback(*args[1:], **kwargs)


def assertRaises(self, expected_exception, *args, **kwargs):
    return _boxedAssertion(self, _BoxedRaisesContext, "assertRaises", expected_exception, None, args, kwargs)


def assertRaisesRegex(self, expected_exception, expected_regex, *args, **kwargs):
    regex = re.compile(expected_regex) if expected_regex is not None else None
    return _boxedAssertion(self, _BoxedRaisesContext, "assertRaisesRegex", expected_exception, regex, args, kwargs)


def assertWarns(self, expected_warning, *args, **kwargs):
    return _boxedAssertion(self, _AssertWarnsContext, "assertWarns", expected_warning, None, args, kwargs)


def _assertNotWarns(self, expected_warning, *args, **kwargs):
    """The opposite of assertWarns. Private due to low demand."""
    return _boxedAssertion(self, _AssertNotWarnsContext, "_assertNotWarns", expected_warning, None, args, kwargs)


def assertWarnsRegex(self, expected_warning, expected_regex, *args, **kwargs):
    regex = re.compile(expected_regex) if expected_regex is not None else None
    return _boxedAssertion(self, _AssertWarnsContext, "assertWarnsRegex", expected_warning, regex, args, kwargs)


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


def assertNotRegex(self, text, unexpected_regex, msg=None):
    """Fail the test if the text matches the regular expression."""
    if isinstance(unexpected_regex, (str, bytes)):
        unexpected_regex = re.compile(unexpected_regex)
    match = unexpected_regex.search(text)
    if match:
        standardMsg = 'Regex matched: %r matches %r in %r' % (
            text[match.start() : match.end()],
            unexpected_regex.pattern,
            text)
        # _formatMessage ensures the longMessage option is respected
        msg = self._formatMessage(msg, standardMsg)
        raise self.failureException(msg)

TestCase.assertRegex = assertRegex
TestCase.assertNotRegex = assertNotRegex
TestCase.assertRaisesRegex = assertRaisesRegex
TestCase._formatMessage = _boxed_format_message
TestCase.assertRaises = assertRaises
TestCase._assertNotWarns = _assertNotWarns
TestCase.assertWarns = assertWarns
TestCase.assertWarnsRegex = assertWarnsRegex
del assertRegex, assertNotRegex, assertRaises, assertRaisesRegex, _assertNotWarns, assertWarns, assertWarnsRegex
