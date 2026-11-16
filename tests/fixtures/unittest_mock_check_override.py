from unittest.mock import Mock, NonCallableMock

descriptor = NonCallableMock.__dict__["assert_called_once_with"]
assert descriptor.__qualname__ == "NonCallableMock.assert_called_once_with"
assert descriptor.__annotations__ == {}


class Checked(Mock):
    def assert_called_with(self, value, *, flag):
        raise AssertionError("original checker")


events = []


class Name:
    def __str__(self):
        events.append("name")
        return "test"


def calls():
    events.append("calls")
    return ""


target = Checked(name=Name())
target._calls_repr = calls
try:
    target.assert_called_once_with()
except AssertionError as error:
    assert str(error) == "Expected 'test' to be called once. Called 0 times."
assert events == ["name", "calls"]
target("before")
answer = object()


def replace_checker():
    def check(self, value, *, flag):
        assert self is target and value == "after" and flag
        return answer

    Checked.assert_called_with = check
    return "after"


assert target.assert_called_once_with(replace_checker(), flag=True) is answer
