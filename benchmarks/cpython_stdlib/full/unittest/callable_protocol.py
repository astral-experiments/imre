"""Check callable assertRaises ordering, failures and exception identity against CPython."""
import unittest


class MyFailure(AssertionError):
    pass


class MyCase(unittest.TestCase):
    failureException = MyFailure


def mark(log: list[str], value: str) -> str:
    log.append(value)
    return value


class Worker:
    def run(self, log: list[str], value: str, *, msg: str) -> None:
        log.append(value + ":" + msg)
        raise KeyError("callback")


def worker(log: list[str]) -> Worker:
    log.append("receiver")
    return Worker()


def argument(log: list[str]) -> str:
    log.append("argument")
    raise KeyError("outside callback")


def echo(log: list[str], value: str) -> None:
    log.append(value)


def throw(original: Exception) -> None:
    raise original


def order(case: unittest.TestCase, log: list[str]) -> None:
    result = case.assertRaises((LookupError, ValueError), worker(log).run,
                               log, mark(log, "pos"), msg=mark(log, "kw"))
    print(result is None)
    print(",".join(log))


def argument_exception(case: unittest.TestCase, log: list[str]) -> None:
    try:
        case.assertRaises(KeyError, echo, log, argument(log))
    except KeyError as outside:
        print(str(outside))
    else:
        raise RuntimeError("caught an argument-evaluation exception")


def invalid_expected(case: unittest.TestCase, log: list[str]) -> None:
    try:
        case.assertRaises(12, echo, log, mark(log, "invalid-type"))
    except TypeError as invalid:
        print(str(invalid))
    else:
        raise RuntimeError("accepted an invalid expected exception")


def crossed_class(case: unittest.TestCase, original: Exception) -> None:
    try:
        case.assertRaises(ValueError, throw, original)
    except KeyError as crossed:
        print(crossed is original)
    else:
        raise RuntimeError("caught an unrelated exception")


def empty_tuple(case: unittest.TestCase, original: Exception) -> None:
    try:
        case.assertRaises((), throw, original)
    except KeyError as empty:
        print(empty is original)
    else:
        raise RuntimeError("empty tuple caught an exception")


def missing(custom: MyCase, log: list[str]) -> None:
    try:
        custom.assertRaises((KeyError, ValueError), echo, log, "called")
    except MyFailure as exc:
        print(str(exc))
    else:
        raise RuntimeError("did not use the case's failureException")


def noncallable(case: unittest.TestCase, log: list[str]) -> None:
    case.assertRaises(TypeError, None, mark(log, "none"))
    try:
        case.assertRaises(ValueError, 17, mark(log, "int"))
    except TypeError as exc:
        print(str(exc))
    else:
        raise RuntimeError("wrong exception from noncallable")


def lambda_message(case: unittest.TestCase) -> None:
    try:
        case.assertRaises(ValueError, lambda: 0)
    except AssertionError as exc:
        print(str(exc))
    else:
        raise RuntimeError("a returning lambda swallowed a failure")


def duplicate_category(case: unittest.TestCase, log: list[str]) -> None:
    try:
        case.assertRaises(ValueError, echo, log, mark(log, "duplicate"), expected_exception=KeyError)
    except TypeError as exc:
        print(str(exc))
    else:
        raise RuntimeError("duplicate expected_exception was forwarded to the callback")


def captured(case: unittest.TestCase, log: list[str], original: Exception) -> None:
    case.assertRaises(KeyError, lambda: throw(original))
    try:
        case.assertRaises(ValueError, lambda: throw(original))
    except KeyError as crossed:
        print(crossed is original)
    try:
        case.assertRaises(KeyError, lambda: log.append("captured once"))
    except AssertionError as returned:
        print(str(returned))
    try:
        case.assertRaises(12, lambda: log.append("must not run"))
    except TypeError as bad:
        print(str(bad))


case = unittest.TestCase()
log: list[str] = []
order(case, log)
argument_exception(case, log)
invalid_expected(case, log)
original = KeyError("preserved")
crossed_class(case, original)
empty_tuple(case, original)
missing(MyCase(), log)
noncallable(case, log)
lambda_message(case)
duplicate_category(case, log)
captured(case, log, original)
print(",".join(log))
