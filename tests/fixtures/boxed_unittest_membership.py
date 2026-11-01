from unittest import TestCase


events = []


class Container:
    def __contains__(self, member):
        events.append(member)
        return member == "present"

    def __repr__(self):
        events.append("repr")
        return "Container()"


case = TestCase()
container = Container()
case.assertIn("present", container)
case.assertNotIn("absent", container)
assert events == ["present", "absent"]
try:
    case.assertNotIn("present", container, "detail")
except AssertionError as exc:
    assert str(exc) == "'present' unexpectedly found in Container() : detail"
else:
    raise AssertionError("membership assertion did not fail")
assert events == ["present", "absent", "present", "repr"]
