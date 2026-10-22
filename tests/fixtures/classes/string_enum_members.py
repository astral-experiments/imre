import enum


class Status(enum.StrEnum):
    READY = enum.auto()
    ALIAS = READY
    SAME = "ready"
    EMPTY = ""
    LABEL = "é\0marker"


class Other(enum.StrEnum):
    READY = "ready"


def observe(value: Status | None, other: Status | None) -> None:
    print(value is other)
    print(value == other)
    print(value is not Other.READY)
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


for member in [Status.READY, Status.ALIAS, Status.SAME, Status.EMPTY, Status.LABEL]:
    observe(member, Status.READY)
    print(echo(member.value))
    print(echo(str(member)))
observe(None, None)
