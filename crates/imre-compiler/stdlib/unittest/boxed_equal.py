# Fixed-layout programs specialize assertEqual in the frontend. Runtime-loaded
# test classes compare the actual objects, including reflected __eq__.
def _baseAssertEqual(self, first, second, msg=None):
    if not first == second:
        difference = _inequality_message(_safe_repr(first), _safe_repr(second))
        msg = self._formatMessage(msg, difference)
        raise self.failureException(msg)


_text_equal = TestCase.assertMultiLineEqual


def assertMultiLineEqual(self, first, second, msg=None):
    self.assertIsInstance(first, str, "First argument is not a string")
    self.assertIsInstance(second, str, "Second argument is not a string")
    return _text_equal(self, first, second, msg)


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


def assertNotEqual(self, first, second, msg=None):
    if not first != second:
        msg = self._formatMessage(msg, f"{_safe_repr(first)!s} == {_safe_repr(second)!s}")
        raise self.failureException(msg)


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
TestCase.assertMultiLineEqual = assertMultiLineEqual
TestCase.assertEqual = assertEqual
assertDictEqual.__qualname__ = "TestCase.assertDictEqual"
TestCase.assertDictEqual = assertDictEqual
assertSetEqual.__qualname__ = "TestCase.assertSetEqual"
TestCase.assertSetEqual = assertSetEqual
TestCase.assertNotEqual = assertNotEqual
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


# Multiset comparison and diagnostics follow CPython 3.14.5 unittest.util and unittest.case.
def _count_diff_all_purpose(actual, expected):
    'Returns list of (cnt_act, cnt_exp, elem) triples where the counts differ'
    # elements need not be hashable
    s, t = list(actual), list(expected)
    m, n = len(s), len(t)
    NULL = object()
    result = []
    for i, elem in enumerate(s):
        if elem is NULL:
            continue
        cnt_s = cnt_t = 0
        for j in range(i, m):
            if s[j] == elem:
                cnt_s += 1
                s[j] = NULL
        for j, other_elem in enumerate(t):
            if other_elem == elem:
                cnt_t += 1
                t[j] = NULL
        if cnt_s != cnt_t:
            diff = (cnt_s, cnt_t, elem)
            result.append(diff)

    for i, elem in enumerate(t):
        if elem is NULL:
            continue
        cnt_t = 0
        for j in range(i, n):
            if t[j] == elem:
                cnt_t += 1
                t[j] = NULL
        diff = (0, cnt_t, elem)
        result.append(diff)
    return result


def _count_diff_hashable(actual, expected):
    'Returns list of (cnt_act, cnt_exp, elem) triples where the counts differ'
    # elements must be hashable
    from collections import Counter
    s, t = Counter(actual), Counter(expected)
    result = []
    for elem, cnt_s in s.items():
        cnt_t = t.get(elem, 0)
        if cnt_s != cnt_t:
            diff = (cnt_s, cnt_t, elem)
            result.append(diff)
    for elem, cnt_t in t.items():
        if elem not in s:
            diff = (0, cnt_t, elem)
            result.append(diff)
    return result


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
