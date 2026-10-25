"""Check native membership decisions, failure messages, and argument order."""
import unittest


class SpecificFailure(AssertionError):
    pass


class Subject(unittest.TestCase):
    failureException = SpecificFailure


def value(log: list[str], label: str, result: str) -> str:
    log.append(label)
    return result


def verify(case: Subject, log: list[str]) -> None:
    case.assertIn("é", "café")
    case.assertIn(3, [1, 3])
    case.assertIn(1, {1: "one"})
    case.assertIn(True, {1: "one"})
    case.assertNotIn("1", {1: "one"})
    case.assertNotIn(2, {})
    case.assertNotIn("z", ["a", "b"])
    case.assertNotIn(7, [])
    try:
        case.assertNotIn(container=value(log, "container", "café"), msg=value(log, "msg", "notice"),
                         member=value(log, "member", "é"))
    except SpecificFailure as caught:
        print(str(caught))
    print(",".join(log))
    try:
        case.assertIn(9, [1, 2, 1])
    except SpecificFailure as caught:
        print(str(caught))
    try:
        case.assertIn("missing", {"b": "two", "a": "one"})
    except SpecificFailure as caught:
        print(str(caught))
    try:
        case.assertNotIn("x", ["x", "z"])
    except SpecificFailure as caught:
        print(str(caught))
    case.longMessage = False
    try:
        case.assertIn("prefix", "", "short")
    except SpecificFailure as caught:
        print(str(caught))


class Override(unittest.TestCase):
    def assertIn(self, member: object, container: dict[int, str], msg: str | None = None) -> None:
        print("override " + repr(member) + " " + repr(container))

    def assertNotIn(self, member: str, container: str, msg: str | None = None) -> None:
        print("override not in " + member + container)


def through_base(case: unittest.TestCase) -> None:
    case.assertIn(1, {1: "one"})
    case.assertNotIn("a", "b")


class CustomFail(unittest.TestCase):
    def fail(self, msg: str | None = None) -> None:
        if msg is None:
            print("custom fail: None")
        else:
            print("custom fail: " + msg)


verify(Subject(), [])
Override().assertIn(1, {1: "one"})
through_base(Override())
CustomFail().assertNotIn("x", "next")
