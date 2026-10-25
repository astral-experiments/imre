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
    def assertIn(self, member: str, container: str, msg: str | None = None) -> None:
        print("override " + member + container)


verify(Subject(), [])
Override().assertIn("a", "b")
