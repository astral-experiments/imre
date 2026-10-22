import enum
from typing import Callable


class Status(enum.StrEnum):
    READY = enum.auto()
    ALIAS = READY
    SAME = "ready"
    EMPTY = ""
    LABEL = "é\0marker"

    def choose(self, override: "Status | None") -> "Status":
        return self if override is None else override


class Other(enum.StrEnum):
    READY = "ready"


def observe(value: Status | None, other: Status | None) -> None:
    print(value is other)
    print(value == other)
    print(value is not Other.READY)
    print(isinstance(value, enum.StrEnum))
    if value is not None:
        print(value.name)
        print(repr(value))
        print(f"{value!s}|{value!r}")
        print(value == Other.READY)
        print(value == "ready")
        print(value != "ready")
        print(isinstance(value, str))
        print(bool(value))


def echo(value: str) -> str:
    return repr(value)


def invoke(callback: Callable[[Status | None], Status], override: Status | None) -> Status:
    return callback(override)


class Picker:
    def __init__(self) -> None:
        pass

    def choose(self, override: Status | None) -> Status:
        return Status.LABEL if override is None else override


def observe_callback(callback: Callable[[Status | None], Status]) -> None:
    print(invoke(callback, None).name)
    print(invoke(callback, Status.LABEL) is Status.LABEL)


for member in [Status.READY, Status.ALIAS, Status.SAME, Status.EMPTY, Status.LABEL]:
    observe(member, Status.READY)
    print(echo(member.value))
    print(echo(str(member)))
observe(None, None)
print(isinstance("ready", enum.StrEnum))


# A scalar enum and an owned record share one dynamic callback signature.
observe_callback(Status.ALIAS.choose)
observe_callback(Status.EMPTY.choose)
observe_callback(Picker().choose)
