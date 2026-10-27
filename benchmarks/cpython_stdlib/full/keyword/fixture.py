"""Native assertion and cleanup fixture for bodies the default runner cannot start."""
import keyword


class Raises:
    def __init__(self, expected):
        self.expected = expected

    def __enter__(self):
        return self

    def __exit__(self, kind, value, tb):
        if kind is None:
            raise AssertionError("exception not raised")
        return issubclass(kind, self.expected)


class Fixture:
    def __init__(self):
        self.cleanups = []

    def addCleanup(self, callback, *args):
        self.cleanups.append((callback, args))

    def doCleanups(self):
        while self.cleanups:
            callback, args = self.cleanups.pop()
            callback(*args)

    def assertTrue(self, value):
        if not value:
            raise AssertionError("expected true")

    def assertFalse(self, value):
        if value:
            raise AssertionError("expected false")

    def assertIn(self, member, container):
        if member not in container:
            raise AssertionError("missing member")

    def assertListEqual(self, left, right):
        if left != right:
            raise AssertionError("different lists")

    def assertRaises(self, expected):
        return Raises(expected)
