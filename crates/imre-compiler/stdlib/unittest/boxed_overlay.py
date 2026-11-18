# Keep runtime Python signatures unannotated while checked callers use source contracts.
_safe_repr.__annotations__ = {}
TestCase._formatMessage.__annotations__ = {}
TestCase.fail.__annotations__ = {}
TestCase.assertNotEqual.__annotations__ = {}
_baseAssertEqual.__annotations__ = {}
assertMultiLineEqual = TestCase.assertMultiLineEqual
# Keep the public Python signature while fixed callers use the class declaration.
assertMultiLineEqual.__annotations__ = {}


def addTypeEqualityFunc(self, typeobj, function):
    """Register a comparison for assertEqual when both operands have this exact type."""
    self._type_equality_funcs[typeobj] = function


def _getAssertEqualityFunc(self, first, second):
    if type(first) is type(second):
        asserter = self._type_equality_funcs.get(type(first))
        if asserter is not None:
            if isinstance(asserter, str):
                return getattr(self, asserter)
            return asserter
    return self._baseAssertEqual


def assertEqual(self, first, second, msg=None):
    assertion_func = self._getAssertEqualityFunc(first, second)
    assertion_func(first, second, msg=msg)


def assertDictEqual(self, d1, d2, msg=None):
    self.assertIsInstance(d1, dict, 'First argument is not a dictionary')
    self.assertIsInstance(d2, dict, 'Second argument is not a dictionary')

    if d1 != d2:
        import pprint
        from unittest.util import _common_shorten_repr

        standardMsg = '%s != %s' % _common_shorten_repr(d1, d2)
        diff = ('\n' + '\n'.join(difflib.ndiff(
                       pprint.pformat(d1).splitlines(),
                       pprint.pformat(d2).splitlines())))
        standardMsg = self._truncateMessage(standardMsg, diff)
        self.fail(self._formatMessage(msg, standardMsg))


def assertSetEqual(self, set1, set2, msg=None):
    """A set-specific equality assertion.

    Args:
        set1: The first set to compare.
        set2: The second set to compare.
        msg: Optional message to use on failure instead of a list of
                differences.

    assertSetEqual uses ducktyping to support different types of sets, and
    is optimized for sets specifically (parameters must support a
    difference method).
    """
    try:
        difference1 = set1.difference(set2)
    except TypeError as e:
        self.fail('invalid type when attempting set difference: %s' % e)
    except AttributeError as e:
        self.fail('first argument does not support set difference: %s' % e)

    try:
        difference2 = set2.difference(set1)
    except TypeError as e:
        self.fail('invalid type when attempting set difference: %s' % e)
    except AttributeError as e:
        self.fail('second argument does not support set difference: %s' % e)

    if not (difference1 or difference2):
        return

    lines = []
    if difference1:
        lines.append('Items in the first set but not the second:')
        for item in difference1:
            lines.append(repr(item))
    if difference2:
        lines.append('Items in the second set but not the first:')
        for item in difference2:
            lines.append(repr(item))

    standardMsg = '\n'.join(lines)
    self.fail(self._formatMessage(msg, standardMsg))


def assertLess(self, a, b, msg=None):
    """Just like self.assertTrue(a < b), but with a nicer default message."""
    if not a < b:
        standardMsg = '%s not less than %s' % (_safe_repr(a), _safe_repr(b))
        self.fail(self._formatMessage(msg, standardMsg))


def assertLessEqual(self, a, b, msg=None):
    """Just like self.assertTrue(a <= b), but with a nicer default message."""
    if not a <= b:
        standardMsg = '%s not less than or equal to %s' % (_safe_repr(a), _safe_repr(b))
        self.fail(self._formatMessage(msg, standardMsg))


def assertGreater(self, a, b, msg=None):
    """Just like self.assertTrue(a > b), but with a nicer default message."""
    if not a > b:
        standardMsg = '%s not greater than %s' % (_safe_repr(a), _safe_repr(b))
        self.fail(self._formatMessage(msg, standardMsg))


def assertGreaterEqual(self, a, b, msg=None):
    """Just like self.assertTrue(a >= b), but with a nicer default message."""
    if not a >= b:
        standardMsg = '%s not greater than or equal to %s' % (_safe_repr(a), _safe_repr(b))
        self.fail(self._formatMessage(msg, standardMsg))


def assertIsInstance(self, obj, cls, msg=None):
    """Same as self.assertTrue(isinstance(obj, cls)), with a nicer
    default message."""
    if not isinstance(obj, cls):
        if isinstance(cls, tuple):
            standardMsg = f'{_safe_repr(obj)} is not an instance of any of {cls!r}'
        else:
            standardMsg = f'{_safe_repr(obj)} is not an instance of {cls!r}'
        self.fail(self._formatMessage(msg, standardMsg))


def assertNotIsInstance(self, obj, cls, msg=None):
    """Included for symmetry with assertIsInstance."""
    if isinstance(obj, cls):
        if isinstance(cls, tuple):
            for x in cls:
                if isinstance(obj, x):
                    cls = x
                    break
        standardMsg = f'{_safe_repr(obj)} is an instance of {cls!r}'
        self.fail(self._formatMessage(msg, standardMsg))


def assertIsSubclass(self, cls, superclass, msg=None):
    try:
        if issubclass(cls, superclass):
            return
    except TypeError:
        if not isinstance(cls, type):
            self.fail(self._formatMessage(msg, f'{cls!r} is not a class'))
        raise
    if isinstance(superclass, tuple):
        standardMsg = f'{cls!r} is not a subclass of any of {superclass!r}'
    else:
        standardMsg = f'{cls!r} is not a subclass of {superclass!r}'
    self.fail(self._formatMessage(msg, standardMsg))


def assertNotIsSubclass(self, cls, superclass, msg=None):
    try:
        if not issubclass(cls, superclass):
            return
    except TypeError:
        if not isinstance(cls, type):
            self.fail(self._formatMessage(msg, f'{cls!r} is not a class'))
        raise
    if isinstance(superclass, tuple):
        for x in superclass:
            if issubclass(cls, x):
                superclass = x
                break
    standardMsg = f'{cls!r} is a subclass of {superclass!r}'
    self.fail(self._formatMessage(msg, standardMsg))


def assertHasAttr(self, obj, name, msg=None):
    if not hasattr(obj, name):
        if isinstance(obj, ModuleType):
            standardMsg = f'module {obj.__name__!r} has no attribute {name!r}'
        elif isinstance(obj, type):
            standardMsg = f'type object {obj.__name__!r} has no attribute {name!r}'
        else:
            standardMsg = f'{type(obj).__name__!r} object has no attribute {name!r}'
        self.fail(self._formatMessage(msg, standardMsg))


def assertNotHasAttr(self, obj, name, msg=None):
    if hasattr(obj, name):
        if isinstance(obj, ModuleType):
            standardMsg = f'module {obj.__name__!r} has unexpected attribute {name!r}'
        elif isinstance(obj, type):
            standardMsg = f'type object {obj.__name__!r} has unexpected attribute {name!r}'
        else:
            standardMsg = f'{type(obj).__name__!r} object has unexpected attribute {name!r}'
        self.fail(self._formatMessage(msg, standardMsg))


def assertIs(self, expr1, expr2, msg=None):
    if expr1 is not expr2:
        _raise_identity_failure(self, _safe_repr(expr1), _safe_repr(expr2), msg)


def assertIsNot(self, expr1, expr2, msg=None):
    if expr1 is expr2:
        _raise_identity_failure(self, _safe_repr(expr1), None, msg)


addTypeEqualityFunc.__qualname__ = "TestCase.addTypeEqualityFunc"
TestCase.addTypeEqualityFunc = addTypeEqualityFunc
TestCase._getAssertEqualityFunc = _getAssertEqualityFunc
TestCase._baseAssertEqual = _baseAssertEqual
TestCase.assertEqual = assertEqual
assertDictEqual.__qualname__ = "TestCase.assertDictEqual"
TestCase.assertDictEqual = assertDictEqual
assertSetEqual.__qualname__ = "TestCase.assertSetEqual"
TestCase.assertSetEqual = assertSetEqual
TestCase.assertLess = assertLess
TestCase.assertLessEqual = assertLessEqual
TestCase.assertGreater = assertGreater
TestCase.assertGreaterEqual = assertGreaterEqual
TestCase.assertIsInstance = assertIsInstance
TestCase.assertNotIsInstance = assertNotIsInstance
TestCase.assertIsSubclass = assertIsSubclass
TestCase.assertNotIsSubclass = assertNotIsSubclass
assertHasAttr.__qualname__ = "TestCase.assertHasAttr"
TestCase.assertHasAttr = assertHasAttr
TestCase.assertNotHasAttr = assertNotHasAttr
TestCase.assertIs = assertIs
TestCase.assertIsNot = assertIsNot


# From CPython 3.14.5 Lib/unittest/case.py. See ../docstrings/LICENSE.
def assertSequenceEqual(self, seq1, seq2, msg=None, seq_type=None):
    """An equality assertion for ordered sequences (like lists and tuples).

    For the purposes of this function, a valid ordered sequence type is one
    which can be indexed, has a length, and has an equality operator.

    Args:
        seq1: The first sequence to compare.
        seq2: The second sequence to compare.
        seq_type: The expected datatype of the sequences, or None if no
                datatype should be enforced.
        msg: Optional message to use on failure instead of a list of
                differences.
    """
    from unittest.util import safe_repr, _common_shorten_repr

    if seq_type is not None:
        seq_type_name = seq_type.__name__
        if not isinstance(seq1, seq_type):
            raise self.failureException('First sequence is not a %s: %s'
                                    % (seq_type_name, safe_repr(seq1)))
        if not isinstance(seq2, seq_type):
            raise self.failureException('Second sequence is not a %s: %s'
                                    % (seq_type_name, safe_repr(seq2)))
    else:
        seq_type_name = "sequence"

    differing = None
    try:
        len1 = len(seq1)
    except (TypeError, NotImplementedError):
        differing = 'First %s has no length.    Non-sequence?' % (
                seq_type_name)

    if differing is None:
        try:
            len2 = len(seq2)
        except (TypeError, NotImplementedError):
            differing = 'Second %s has no length.    Non-sequence?' % (
                    seq_type_name)

    if differing is None:
        if seq1 == seq2:
            return

        differing = '%ss differ: %s != %s\n' % (
                (seq_type_name.capitalize(),) +
                _common_shorten_repr(seq1, seq2))

        for i in range(min(len1, len2)):
            try:
                item1 = seq1[i]
            except (TypeError, IndexError, NotImplementedError):
                differing += ('\nUnable to index element %d of first %s\n' %
                             (i, seq_type_name))
                break

            try:
                item2 = seq2[i]
            except (TypeError, IndexError, NotImplementedError):
                differing += ('\nUnable to index element %d of second %s\n' %
                             (i, seq_type_name))
                break

            if item1 != item2:
                differing += ('\nFirst differing element %d:\n%s\n%s\n' %
                             ((i,) + _common_shorten_repr(item1, item2)))
                break
        else:
            if (len1 == len2 and seq_type is None and
                type(seq1) != type(seq2)):
                # The sequences are the same, but have differing types.
                return

        if len1 > len2:
            differing += ('\nFirst %s contains %d additional '
                         'elements.\n' % (seq_type_name, len1 - len2))
            try:
                differing += ('First extra element %d:\n%s\n' %
                              (len2, safe_repr(seq1[len2])))
            except (TypeError, IndexError, NotImplementedError):
                differing += ('Unable to index element %d '
                              'of first %s\n' % (len2, seq_type_name))
        elif len1 < len2:
            differing += ('\nSecond %s contains %d additional '
                         'elements.\n' % (seq_type_name, len2 - len1))
            try:
                differing += ('First extra element %d:\n%s\n' %
                              (len1, safe_repr(seq2[len1])))
            except (TypeError, IndexError, NotImplementedError):
                differing += ('Unable to index element %d '
                              'of second %s\n' % (len1, seq_type_name))
    import pprint
    standardMsg = differing
    diffMsg = '\n' + '\n'.join(
        difflib.ndiff(pprint.pformat(seq1).splitlines(),
                      pprint.pformat(seq2).splitlines()))

    standardMsg = self._truncateMessage(standardMsg, diffMsg)
    msg = self._formatMessage(msg, standardMsg)
    self.fail(msg)


def assertListEqual(self, list1, list2, msg=None):
    """A list-specific equality assertion.

    Args:
        list1: The first list to compare.
        list2: The second list to compare.
        msg: Optional message to use on failure instead of a list of
                differences.

    """
    self.assertSequenceEqual(list1, list2, msg, seq_type=list)


def assertTupleEqual(self, tuple1, tuple2, msg=None):
    """A tuple-specific equality assertion.

    Args:
        tuple1: The first tuple to compare.
        tuple2: The second tuple to compare.
        msg: Optional message to use on failure instead of a list of
                differences.
    """
    self.assertSequenceEqual(tuple1, tuple2, msg, seq_type=tuple)


TestCase.assertSequenceEqual = assertSequenceEqual
TestCase.assertListEqual = assertListEqual
TestCase.assertTupleEqual = assertTupleEqual


from unittest.util import _count_diff_all_purpose, _count_diff_hashable


def assertCountEqual(self, first, second, msg=None):
    """Asserts that two iterables have the same elements, the same number of
    times, without regard to order.

        self.assertEqual(Counter(list(first)),
                         Counter(list(second)))

     Example:
        - [0, 1, 1] and [1, 0, 1] compare equal.
        - [0, 0, 1] and [0, 1] compare unequal.

    """
    from collections import Counter
    first_seq, second_seq = list(first), list(second)
    try:
        first = Counter(first_seq)
        second = Counter(second_seq)
    except TypeError:
        # Handle case with unhashable elements
        differences = _count_diff_all_purpose(first_seq, second_seq)
    else:
        if first == second:
            return
        differences = _count_diff_hashable(first_seq, second_seq)

    if differences:
        standardMsg = 'Element counts were not equal:\n'
        lines = ['First has %d, Second has %d:  %r' % diff for diff in differences]
        diffMsg = '\n'.join(lines)
        standardMsg = self._truncateMessage(standardMsg, diffMsg)
        msg = self._formatMessage(msg, standardMsg)
        self.fail(msg)


TestCase.assertCountEqual = assertCountEqual


def assertIn(self, member, container, msg=None):
    if member not in container:
        standardMsg = '%s not found in %s' % (_safe_repr(member), _safe_repr(container))
        self.fail(self._formatMessage(msg, standardMsg))


def assertNotIn(self, member, container, msg=None):
    if member in container:
        standardMsg = '%s unexpectedly found in %s' % (_safe_repr(member), _safe_repr(container))
        self.fail(self._formatMessage(msg, standardMsg))


TestCase.assertIn = assertIn
TestCase.assertNotIn = assertNotIn


# From CPython 3.14.5, Lib/unittest/case.py.
def assertAlmostEqual(self, first, second, places=None, msg=None,
                      delta=None):
    """Fail if the two objects are unequal as determined by their
       difference rounded to the given number of decimal places
       (default 7) and comparing to zero, or by comparing that the
       difference between the two objects is more than the given
       delta.

       Note that decimal places (from zero) are usually not the same
       as significant digits (measured from the most significant digit).

       If the two objects compare equal then they will automatically
       compare almost equal.
    """
    if first == second:
        # shortcut
        return
    if delta is not None and places is not None:
        raise TypeError("specify delta or places not both")

    diff = abs(first - second)
    if delta is not None:
        if diff <= delta:
            return

        standardMsg = '%s != %s within %s delta (%s difference)' % (
            _safe_repr(first),
            _safe_repr(second),
            _safe_repr(delta),
            _safe_repr(diff))
    else:
        if places is None:
            places = 7

        if round(diff, places) == 0:
            return

        standardMsg = '%s != %s within %r places (%s difference)' % (
            _safe_repr(first),
            _safe_repr(second),
            places,
            _safe_repr(diff))
    msg = self._formatMessage(msg, standardMsg)
    raise self.failureException(msg)


def assertNotAlmostEqual(self, first, second, places=None, msg=None,
                         delta=None):
    """Fail if the two objects are equal as determined by their
       difference rounded to the given number of decimal places
       (default 7) and comparing to zero, or by comparing that the
       difference between the two objects is less than the given delta.

       Note that decimal places (from zero) are usually not the same
       as significant digits (measured from the most significant digit).

       Objects that are equal automatically fail.
    """
    if delta is not None and places is not None:
        raise TypeError("specify delta or places not both")
    diff = abs(first - second)
    if delta is not None:
        if not (first == second) and diff > delta:
            return
        standardMsg = '%s == %s within %s delta (%s difference)' % (
            _safe_repr(first),
            _safe_repr(second),
            _safe_repr(delta),
            _safe_repr(diff))
    else:
        if places is None:
            places = 7
        if not (first == second) and round(diff, places) != 0:
            return
        standardMsg = '%s == %s within %r places' % (_safe_repr(first),
                                                     _safe_repr(second),
                                                     places)

    msg = self._formatMessage(msg, standardMsg)
    raise self.failureException(msg)


TestCase.assertAlmostEqual = assertAlmostEqual
TestCase.assertNotAlmostEqual = assertNotAlmostEqual


# From CPython 3.14.5 unittest.case. See ../docstrings/LICENSE.
def _tail_type_check(self, s, tails, msg):
    if not isinstance(tails, tuple):
        tails = (tails,)
    for tail in tails:
        if isinstance(tail, str):
            if not isinstance(s, str):
                self.fail(self._formatMessage(msg,
                        f'Expected str, not {type(s).__name__}'))
        elif isinstance(tail, (bytes, bytearray)):
            if not isinstance(s, (bytes, bytearray)):
                self.fail(self._formatMessage(msg,
                        f'Expected bytes, not {type(s).__name__}'))


def assertStartsWith(self, s, prefix, msg=None):
    try:
        if s.startswith(prefix):
            return
    except (AttributeError, TypeError):
        self._tail_type_check(s, prefix, msg)
        raise
    from unittest.util import safe_repr
    a = safe_repr(s, short=True)
    b = safe_repr(prefix)
    if isinstance(prefix, tuple):
        standardMsg = f"{a} doesn't start with any of {b}"
    else:
        standardMsg = f"{a} doesn't start with {b}"
    self.fail(self._formatMessage(msg, standardMsg))


def assertNotStartsWith(self, s, prefix, msg=None):
    try:
        if not s.startswith(prefix):
            return
    except (AttributeError, TypeError):
        self._tail_type_check(s, prefix, msg)
        raise
    if isinstance(prefix, tuple):
        for x in prefix:
            if s.startswith(x):
                prefix = x
                break
    from unittest.util import safe_repr
    a = safe_repr(s, short=True)
    b = safe_repr(prefix)
    self.fail(self._formatMessage(msg, f"{a} starts with {b}"))


def assertEndsWith(self, s, suffix, msg=None):
    try:
        if s.endswith(suffix):
            return
    except (AttributeError, TypeError):
        self._tail_type_check(s, suffix, msg)
        raise
    from unittest.util import safe_repr
    a = safe_repr(s, short=True)
    b = safe_repr(suffix)
    if isinstance(suffix, tuple):
        standardMsg = f"{a} doesn't end with any of {b}"
    else:
        standardMsg = f"{a} doesn't end with {b}"
    self.fail(self._formatMessage(msg, standardMsg))


def assertNotEndsWith(self, s, suffix, msg=None):
    try:
        if not s.endswith(suffix):
            return
    except (AttributeError, TypeError):
        self._tail_type_check(s, suffix, msg)
        raise
    if isinstance(suffix, tuple):
        for x in suffix:
            if s.endswith(x):
                suffix = x
                break
    from unittest.util import safe_repr
    a = safe_repr(s, short=True)
    b = safe_repr(suffix)
    self.fail(self._formatMessage(msg, f"{a} ends with {b}"))


TestCase._tail_type_check = _tail_type_check
TestCase.assertStartsWith = assertStartsWith
TestCase.assertNotStartsWith = assertNotStartsWith
TestCase.assertEndsWith = assertEndsWith
TestCase.assertNotEndsWith = assertNotEndsWith

# Exception classes and callable arguments use the boxed Python object protocol.
import traceback as _traceback


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
TestCase.assertRaises = assertRaises
TestCase._assertNotWarns = _assertNotWarns
TestCase.assertWarns = assertWarns
TestCase.assertWarnsRegex = assertWarnsRegex
del assertRegex, assertNotRegex, assertRaises, assertRaisesRegex, _assertNotWarns, assertWarns, assertWarnsRegex

# A cleanup may be any callable, with its original positional and keyword arguments.
def addCleanup(self, function, /, *args, **kwargs):
    if not hasattr(self, "_cleanups"):
        self._cleanups = []
    self._cleanups.append((function, args, kwargs))


def _enterCleanupContext(cm, register):
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
    register(exit, cm, None, None, None)
    return result


def enterContext(self, cm):
    return _enterCleanupContext(cm, self.addCleanup)


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

def _validateCaseMethod(case, methodName):
    case._outcome = None
    case._testMethodDoc = "No test"
    try:
        method = getattr(case, methodName)
    except AttributeError:
        if methodName != "runTest":
            raise ValueError("no such test method in %s: %s" % (case.__class__, methodName))
    else:
        case._testMethodDoc = method.__doc__
    case._cleanups = []
    case._subtest = None
    case._type_equality_funcs = {}
    case.addTypeEqualityFunc(dict, "assertDictEqual")
    case.addTypeEqualityFunc(list, "assertListEqual")
    case.addTypeEqualityFunc(tuple, "assertTupleEqual")
    case.addTypeEqualityFunc(set, "assertSetEqual")
    case.addTypeEqualityFunc(frozenset, "assertSetEqual")
    case.addTypeEqualityFunc(str, "assertMultiLineEqual")


def _boxedCheckFixtures(cls):
    if not issubclass(cls, TestCase):
        for name in ("addClassCleanup", "enterClassContext", "doClassCleanups"):
            if getattr(cls, name, None) is not None:
                raise NotImplementedError("native unittest does not yet support class cleanup hooks: " + name)


def _caseRun(self, result=None):
    stopTestRun = None
    if result is None:
        result = self.defaultTestResult()
        startTestRun = getattr(result, "startTestRun", None)
        stopTestRun = getattr(result, "stopTestRun", None)
        if startTestRun is not None:
            startTestRun()
    result.startTest(self)
    try:
        cls = type(self)
        method = getattr(self, self._testMethodName)
        if getattr(cls, "__unittest_skip__", False) or getattr(method, "__unittest_skip__", False):
            reason = getattr(cls, "__unittest_skip_why__", "") or getattr(method, "__unittest_skip_why__", "")
            case._addSkip(result, self, reason)
            return result
        expecting_failure = (getattr(self, "__unittest_expecting_failure__", False)
                             or getattr(method, "__unittest_expecting_failure__", False))
        outcome = case._Outcome(result)
        start_time = case.time.perf_counter()
        try:
            self._outcome = outcome
            with outcome.testPartExecutor(self):
                self._callSetUp()
            if outcome.success:
                outcome.expecting_failure = expecting_failure
                with outcome.testPartExecutor(self):
                    self._callTestMethod(method)
                outcome.expecting_failure = False
                with outcome.testPartExecutor(self):
                    self._callTearDown()
            self.doCleanups()
            self._addDuration(result, case.time.perf_counter() - start_time)
            if outcome.success:
                if not expecting_failure:
                    result.addSuccess(self)
                elif outcome.expectedFailure is None:
                    self._addUnexpectedSuccess(result)
                else:
                    self._addExpectedFailure(result, outcome.expectedFailure)
        finally:
            outcome.expectedFailure = None
            self._outcome = None
        return result
    finally:
        result.stopTest(self)
        if stopTestRun is not None:
            stopTestRun()


def _runRegistered(result, test, name):
    _caseRun(test, result)


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
        if not outcome.success:
            if getattr(outcome.result, "failfast", False):
                raise case._ShouldStop
        elif outcome.expectedFailure:
            raise case._ShouldStop
        return suppressed
    finally:
        self.case._subtest = self.parent


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


def _callTest(self, *args, **kwargs):
    return self.run(*args, **kwargs)


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
        return
    try:
        hook()
    except Exception as exc:
        if isinstance(result, _DebugResult):
            raise
        _boxedFixtureProblem(parent, method, result, exc, exc.__traceback__)


def _boxedFixtureProblem(parent, method, result, exc, tb):
    label = method + " (" + parent + ")"
    holder = _ErrorHolder(label)
    addSkip = getattr(result, "addSkip", None)
    if addSkip is not None and isinstance(exc, SkipTest):
        addSkip(holder, str(exc))
    else:
        result.addError(holder, (type(exc), exc, tb))


def _call_if_exists(result, name):
    getattr(result, name, lambda: None)()


def _boxedClassPhase(cls, method, result):
    hook = getattr(cls, method, None)
    cleanups = getattr(cls, "doClassCleanups", None)
    setup = method == "setUpClass"
    if hook is None and (setup or cleanups is None):
        return
    _call_if_exists(result, "_setupStdout")
    try:
        failed = False
        if hook is not None:
            try:
                hook()
            except Exception as exc:
                if isinstance(result, _DebugResult):
                    raise
                failed = True
                if setup:
                    try:
                        cls._classSetupFailed = True
                    except TypeError:
                        pass
                _boxedFixtureProblem(cls.__module__ + "." + cls.__qualname__, method, result, exc, exc.__traceback__)
        if cleanups is not None and (not setup or failed):
            cleanups()
            for _, exc, tb in cls.tearDown_exceptions:
                if isinstance(result, _DebugResult):
                    raise exc
                _boxedFixtureProblem(cls.__module__ + "." + cls.__qualname__, method, result, exc, tb)
    finally:
        _call_if_exists(result, "_restoreStdout")


def _boxedTearDownClass(cls, result):
    if (cls is not None and not getattr(result, "_moduleSetUpFailed", False)
            and not getattr(cls, "_classSetupFailed", False)
            and not getattr(cls, "__unittest_skip__", False)):
        _boxedClassPhase(cls, "tearDownClass", result)


def _boxedTearDownModule(previous, result):
    if previous is None or getattr(result, "_moduleSetUpFailed", False):
        return
    name = previous.__module__
    try:
        module = sys.modules[name]
    except KeyError:
        return
    _call_if_exists(result, "_setupStdout")
    try:
        _boxedFixturePhase(getattr(module, "tearDownModule", None), "tearDownModule", name, result)
        _boxedFixturePhase(case.doModuleCleanups, "tearDownModule", name, result)
    finally:
        _call_if_exists(result, "_restoreStdout")


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
    hook = getattr(module, "setUpModule", None)
    if hook is None:
        return
    _call_if_exists(result, "_setupStdout")
    try:
        try:
            hook()
        except Exception as exc:
            if isinstance(result, _DebugResult):
                raise
            result._moduleSetUpFailed = True
            _boxedFixtureProblem(name, "setUpModule", result, exc, exc.__traceback__)
        if result._moduleSetUpFailed:
            _boxedFixturePhase(case.doModuleCleanups, "setUpModule", name, result)
    finally:
        _call_if_exists(result, "_restoreStdout")


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
                        _boxedClassPhase(cls, "setUpClass", result)
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

# The fixed-layout loader was defined before the boxed suite classes.
from unittest import suite
from unittest.suite import BaseTestSuite, TestSuite
TestLoader.suiteClass = TestSuite

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
from unittest.signals import installHandler, registerResult, removeHandler, removeResult


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

# Adapted from CPython 3.14.5 Lib/unittest/case.py (PSF-2.0).
from functools import update_wrapper as _update_wrapper
from types import FunctionType as _FunctionType


def _identity(test_item):
    return test_item


def skip(reason):
    def decorator(test_item):
        if not isinstance(test_item, type):
            def skip_wrapper(*args, **kwargs):
                raise SkipTest(reason)
            test_item = _update_wrapper(skip_wrapper, test_item)
        test_item.__unittest_skip__ = True
        test_item.__unittest_skip_why__ = reason
        return test_item

    if isinstance(reason, _FunctionType):
        test_item = reason
        reason = ""
        return decorator(test_item)
    return decorator


def skipIf(condition, reason):
    if condition:
        return skip(reason)
    return _identity


def skipUnless(condition, reason):
    if not condition:
        return skip(reason)
    return _identity


def _boxedSkipTest(self, reason):
    raise SkipTest(reason)


def _boxedSkipCase(self, case, name, reason):
    self.startTest(case)
    try:
        self.addSkip(case, reason)
    finally:
        self.stopTest(case)


TestCase.skipTest = _boxedSkipTest
TestResult._skipCase = _boxedSkipCase

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
                self._original_stdout.write(result._bufferedOutput(output, "Stdout"))
            if error:
                self._original_stderr.write(result._bufferedOutput(error, "Stderr"))
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
    if name in ("stream", "_memory", "__getstate__"):
        raise AttributeError(name)
    return getattr(self.stream, name)


def _boxedRunnerInit(self, stream=None, descriptions=True, verbosity=1, failfast=False,
                     buffer=False, resultclass=None, warnings=None, *, tb_locals=False,
                     durations=None):
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


def _boxedTextResultInit(self, stream=None, descriptions=True, verbosity=1, *, durations=None):
    super(TextTestResult, self).__init__(stream, descriptions, verbosity)
    self.stream = stream if isinstance(stream, _WritelnDecorator) else _WritelnDecorator(stream)
    self.descriptions = descriptions
    self.verbose = verbosity
    self._failureNames = []
    self._errorNames = []
    self.current_name = ""
    self.name_pending = False
    self.started = time.time()
    self.showAll = verbosity > 1
    self.dots = verbosity == 1
    self._newline = True
    self.durations = durations


from unittest import result, runner as _runnerModule


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
TestResult._exc_info_to_string = result._exc_info_to_string
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
