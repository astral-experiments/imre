# Fixed-layout programs specialize assertEqual in the frontend. Runtime-loaded
# test classes compare the actual objects, including reflected __eq__.
def _safe_repr(value):
    try:
        return repr(value)
    except Exception:
        return object.__repr__(value)


def _baseAssertEqual(self, first, second, msg=None):
    if not first == second:
        difference = _inequality_message(_safe_repr(first), _safe_repr(second))
        raise self.failureException(self._formatMessage(msg, difference))


def assertEqual(self, first, second, msg=None):
    if type(first) is str and type(second) is str:
        return self.assertMultiLineEqual(first, second, msg)
    return self._baseAssertEqual(first, second, msg)


def assertNotEqual(self, first, second, msg=None):
    if not first != second:
        msg = self._formatMessage(msg, f"{_safe_repr(first)!s} == {_safe_repr(second)!s}")
        raise self.failureException(msg)


def assertIs(self, expr1, expr2, msg=None):
    if expr1 is not expr2:
        _raise_identity_failure(self, _safe_repr(expr1), _safe_repr(expr2), msg)


def assertIsNot(self, expr1, expr2, msg=None):
    if expr1 is expr2:
        _raise_identity_failure(self, _safe_repr(expr1), None, msg)


TestCase._baseAssertEqual = _baseAssertEqual
TestCase.assertEqual = assertEqual
TestCase.assertNotEqual = assertNotEqual
TestCase.assertIs = assertIs
TestCase.assertIsNot = assertIsNot


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
