"""An inherited exception retains the same object views used by errno and Template."""
import errno
from difflib import ndiff
from string import Template


class Message(Exception):
    def __init__(self, value, *args):
        self.value = value
        super().__init__(value, *args)

    def __str__(self):
        return self.value


class Child(Message):
    pass


def member(name: str) -> object:
    return getattr(errno, name)


def show_list(values: list[str]) -> None:
    error = Child(values)
    mapping = member("errorcode")
    print(error.value is values)
    print(error.args[0] is values)
    print(error.value is None)
    print(error.value == values)
    print(values == error.value)
    print(mapping == values)
    print(values == mapping)
    print(mapping == error.value)
    print(error.value == mapping)
    values.append("second")
    print(error.value[1])
    print(len(error.value))
    print(Template('$value').substitute(value=error.value))
    print(error.args[0] == ["first", "second"])


show_list(["first"])
changes = list(ndiff(["old", "same"], ["new", "same"]))
deferred = Child(changes)
print(len(deferred.value))
print(deferred.value[1])
print(deferred.value == changes)
print(repr(deferred.value) == repr(changes))
metadata = member("__spec__")
error = Child(metadata)
print(error.value is None)
print(repr(error.args[0]) == repr(metadata))
print(repr(error.args) == "(" + repr(metadata) + ",)")
print(error.args == (member("__spec__"),))
try:
    print(str(error))
except TypeError as exc:
    print(str(exc))

huge = Child([10 ** 4300])
try:
    print(Template("$value").substitute(value=huge.value))
except ValueError as exc:
    print(str(exc))
