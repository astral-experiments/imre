"""Differential control for context semantics; this is not an upstream test ID."""
import unittest


class Problem(ValueError):
    pass


class SpecialCase(unittest.TestCase):
    failureException = Problem


def failure_class(case: unittest.TestCase) -> None:
    with case.assertRaises(case.failureException) as caught:
        case.fail("overridden failure constructor")
    print(str(caught.exception))
    original = Problem("original instance")
    with case.assertRaises(Problem) as saved:
        raise original
    print(saved.exception is original)
    try:
        with case.assertRaises(TypeError, msg="from base view"):
            pass
    except Problem as error:
        print(str(error))


def mark(label: str) -> int:
    print(label)
    return 17


class Case(unittest.TestCase):
    def leave(self) -> int:
        with self.assertRaises(ValueError, msg="return"):
            return 5
        return 0

    def exercise(self) -> None:
        ctx = self.assertRaises(((LookupError,), ValueError))
        other = self.assertRaises(expected_exception=TypeError)
        with ctx as captured:
            with self.assertRaises(AttributeError):
                print(captured.exception)
            raise Problem("first")
        with other as captured:
            captured = ctx
            raise TypeError("second")
        print(str(captured.exception))
        print(str(other.exception))
        with ctx:
            raise KeyError("again")
        print(str(ctx.exception))

        try:
            with self.assertRaises((Problem,)):
                pass
        except AssertionError as error:
            print(str(error))
        try:
            with self.assertRaises(()) as empty:
                raise Problem("propagated")
        except ValueError as error:
            print(str(error))
        try:
            with self.assertRaises(ValueError, invalid=mark("keyword")):
                print("unreachable")
        except TypeError as error:
            print(str(error))
        try:
            self.assertRaises((ValueError, mark("first argument")), invalid=mark("last argument"))
        except TypeError as error:
            print(str(error))
        try:
            self.leave()
        except AssertionError as error:
            print(str(error))
        for n in range(2):
            try:
                with self.assertRaises(ValueError, msg="loop"):
                    if n:
                        continue
                    break
            except AssertionError as error:
                print(n)
                print(str(error))


Case().exercise()
failure_class(SpecialCase())

unbound = UnboundLocalError("unassigned")
with unittest.TestCase().assertRaises(NameError) as name_error:
    raise unbound
print(name_error.exception is unbound)
