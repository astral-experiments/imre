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


TestCase._baseAssertEqual = _baseAssertEqual
TestCase.assertEqual = assertEqual
