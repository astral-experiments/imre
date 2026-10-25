"""Differential control for context semantics; this is not an upstream test ID."""
import unittest
import shutil
import tempfile
from pathlib import Path


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

case = unittest.TestCase()
missing = KeyError("key")
with case.assertRaises(LookupError) as lookup:
    with case.assertRaises(NameError):
        raise missing
print(lookup.exception is missing)

special = shutil.SpecialFileError("special")
with case.assertRaises(OSError) as os_error:
    with case.assertRaises(shutil.Error):
        raise special
print(os_error.exception is special)
with case.assertRaises(shutil.SpecialFileError) as exact_special:
    raise special
print(exact_special.exception is special)

with tempfile.TemporaryDirectory() as directory:
    target = Path(directory) / "identity.txt"
    try:
        target.write_text("é", encoding="ascii")
    except UnicodeEncodeError as encoding:
        with case.assertRaises((LookupError, UnicodeError)) as family:
            with case.assertRaises(shutil.Error):
                raise encoding
        with case.assertRaises(UnicodeEncodeError) as exact_encoding:
            raise encoding
        print(family.exception is encoding)
        print(exact_encoding.exception is encoding)
    else:
        raise AssertionError("a strict ASCII write did not raise")
    target.write_text("original")
    try:
        shutil.copyfile(str(target), str(target))
    except shutil.SameFileError as same:
        with case.assertRaises(shutil.Error) as copy_error:
            with case.assertRaises(UnicodeError):
                raise same
        print(copy_error.exception is same)
    else:
        raise AssertionError("copying a file onto itself did not raise")
